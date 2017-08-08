use std::path::PathBuf;
use std::fs::{self, File};
use std::io::{Read, ErrorKind as IoErrorKind};
use std::{mem, time};

use futures::{Future, Stream, Sink, Poll, Async, future};
use futures::sync::mpsc::SendError;

use hyper::{Error, Chunk, Method, StatusCode, Body, header};
use hyper::server::{Service, Request, Response};

use tokio_core::reactor::Handle;

use requested_path::RequestedPath;

pub type ResponseFuture = Box<Future<Item=Response, Error=Error>>;

/// The default upstream service for `Static`.
///
/// Responds with 404 to GET/HEAD, and with 400 to other methods.
pub struct DefaultUpstream;
impl Service for DefaultUpstream {
    type Request = Request;
    type Response = Response;
    type Error = Error;
    type Future = ResponseFuture;

    fn call(&self, req: Self::Request) -> Self::Future {
        future::ok(Response::new().with_status(match req.method() {
            &Method::Head | &Method::Get => StatusCode::NotFound,
            _ => StatusCode::BadRequest,
        })).boxed()
    }
}

/// A stream that produces Hyper chunks from a file.
struct FileChunkStream(File);
impl Stream for FileChunkStream {
    type Item = Result<Chunk, Error>;
    type Error = SendError<Self::Item>;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        // TODO: non-blocking read
        let mut buf: [u8; 16384] = unsafe { mem::uninitialized() };
        match self.0.read(&mut buf) {
            Ok(0) => Ok(Async::Ready(None)),
            Ok(size) => Ok(Async::Ready(Some(Ok(
                Chunk::from(buf[0..size].to_owned())
            )))),
            Err(err) => Ok(Async::Ready(Some(Err(Error::Io(err))))),
        }
    }
}

/// A Hyper service implementing static file serving.
///
/// This service serves files from a single filesystem path, which may be absolute or relative.
/// Incoming requests are mapped onto the filesystem by appending their URL path to the service
/// root path. If the filesystem path corresponds to a regular file, the service will attempt to
/// serve it. Otherwise, if the path corresponds to a directory containing an `index.html`,
/// the service will attempt to serve that instead.
///
/// If the path doesn't match any real object in the filesystem, the service will call the optional
/// upstream service, or respond with 404. Permission errors always result in a 403 response.
///
/// Only `GET` and `HEAD` requests are handled. Requests with a different method are passed to
/// the optional upstream service, or responded to with 400.
///
/// If an IO error occurs whilst attempting to serve a file, `hyper::Error(Io)` will be returned.
#[derive(Clone)]
pub struct Static<U = DefaultUpstream> {
    /// Handle to the Tokio core.
    handle: Handle,
    /// The path this service is serving files from.
    root: PathBuf,
    /// The upstream service to call when the path is not matched.
    upstream: U,
    /// The cache duration in seconds clients should cache files.
    cache_seconds: u32,
}

impl<U> Static<U> {
    /// Create a new instance of `Static` with a given root path and upstream.
    ///
    /// If `Path::new("")` is given, files will be served from the current directory.
    pub fn with_upstream<P: Into<PathBuf>>(handle: &Handle, root: P, upstream: U) -> Self {
        Self {
            handle: handle.clone(),
            root: root.into(),
            upstream: upstream,
            cache_seconds: 0,
        }
    }

    /// Add cache headers to responses for the given duration.
    pub fn with_cache_headers(mut self, seconds: u32) -> Self {
        self.cache_seconds = seconds;
        self
    }
}

impl Static<DefaultUpstream> {
    /// Create a new instance of `Static` with a given root path.
    ///
    /// If `Path::new("")` is given, files will be served from the current directory.
    pub fn new<P: Into<PathBuf>>(handle: &Handle, root: P) -> Self {
        Self::with_upstream(handle, root, DefaultUpstream)
    }
}

impl<U> Service for Static<U>
        where U: Service<
            Request = Request,
            Response = Response,
            Error = Error,
            Future = ResponseFuture
        > {
    type Request = Request;
    type Response = Response;
    type Error = Error;
    type Future = ResponseFuture;

    fn call(&self, req: Request) -> Self::Future {
        // Handle only `GET`/`HEAD` and absolute paths.
        match req.method() {
            &Method::Head | &Method::Get => {},
            _ => return self.upstream.call(req),
        }

        if req.uri().is_absolute() {
            return self.upstream.call(req);
        }

        let requested_path = RequestedPath::new(&self.root, &req);

        let metadata = match fs::metadata(&requested_path.path) {
            Ok(meta) => meta,
            Err(e) => {
                return match e.kind() {
                    IoErrorKind::NotFound => {
                        self.upstream.call(req)
                    },
                    IoErrorKind::PermissionDenied => {
                        future::ok(Response::new().with_status(StatusCode::Forbidden)).boxed()
                    },
                    _ => {
                        future::err(Error::Io(e)).boxed()
                    },
                };
            },
        };

        // If the URL ends in a slash, serve the file directly.
        // Otherwise, redirect to the directory equivalent of the URL.
        if requested_path.should_redirect(&metadata, &req) {
            // Append the trailing slash
            let mut target = req.path().to_owned();
            target.push('/');
            if let Some(query) = req.query() {
                target.push('?');
                target.push_str(query);
            }

            // Perform an HTTP 301 Redirect.
            return future::ok(Response::new()
                .with_status(StatusCode::MovedPermanently)
                .with_header(header::Location::new(target))
            ).boxed();
        }

        // Resolve the directory index, if necessary.
        let (path, metadata) = match requested_path.get_file(metadata) {
            None => return self.upstream.call(req),
            Some(val) => val,
        };

        // Check If-Modified-Since header.
        let modified = match metadata.modified() {
            Ok(time) => time,
            Err(err) => return future::err(Error::Io(err)).boxed(),
        };
        let http_modified = header::HttpDate::from(modified);

        if let Some(&header::IfModifiedSince(ref value)) = req.headers().get() {
            if http_modified <= *value {
                return future::ok(Response::new()
                    .with_status(StatusCode::NotModified)
                ).boxed();
            }
        }

        // Build response headers.
        let size = metadata.len();
        let delta_modified = modified.duration_since(time::UNIX_EPOCH)
            .expect("cannot express mtime as duration since epoch");
        let etag = format!("{0:x}-{1:x}.{2:x}", size, delta_modified.as_secs(), delta_modified.subsec_nanos());
        let mut res = Response::new()
            .with_header(header::ContentLength(size))
            .with_header(header::LastModified(http_modified))
            .with_header(header::ETag(header::EntityTag::weak(etag)));

        if self.cache_seconds != 0 {
            res.headers_mut().set(header::CacheControl(vec![
                header::CacheDirective::Public,
                header::CacheDirective::MaxAge(self.cache_seconds)
            ]));
        }

        // Stream response body.
        match req.method() {
            &Method::Head => {},
            &Method::Get => {
                let file = match File::open(path) {
                    Ok(file) => file,
                    Err(err) => return future::err(Error::Io(err)).boxed(),
                };

                let (sender, body) = Body::pair();
                self.handle.spawn(
                    sender.send_all(FileChunkStream(file))
                        .map(|_| ())
                        .map_err(|_| ())
                );
                res.set_body(body);
            },
            _ => unreachable!(),
        }

        future::ok(res).boxed()
    }
}
