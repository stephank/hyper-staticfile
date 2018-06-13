use std::path::PathBuf;
use std::fs;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::mem;

use chrono::{DateTime, SubsecRound};
use chrono::offset::{Local as LocalTz};

use futures::{Async, Future, Poll, Stream, future};

use http::{Method, StatusCode, header};
use http::response::Builder as ResponseBuilder;

use hyper::{Body, Chunk};
use hyper::service::Service;

use tokio::fs::File;
use tokio::io::AsyncRead;

use requested_path::RequestedPath;

type Request = ::http::Request<Body>;
type Response = ::http::Response<Body>;
type ResponseFuture = Box<Future<Item=Response, Error=String> + Send + 'static>;

/// The default upstream service for `Static`.
///
/// Responds with 404 to GET/HEAD, and with 400 to other methods.
pub struct DefaultUpstream;

impl Service for DefaultUpstream {
    type ReqBody = Body;
    type ResBody = Body;
    type Error = String;
    type Future = ResponseFuture;

    fn call(&mut self, req: Request) -> ResponseFuture {
        Box::new(future::result(
            ResponseBuilder::new()
                .status(match req.method() {
                    &Method::HEAD | &Method::GET => StatusCode::NOT_FOUND,
                    _ => StatusCode::BAD_REQUEST,
                })
                .body(Body::empty())
                .map_err(|e| e.to_string())
        ))
    }
}

/// Wrap a File into a stream of chunks.
struct FileChunkStream {
    file: File,
    buf: Box<[u8; BUF_SIZE]>,
}

impl FileChunkStream {
    pub fn new(file: File) -> FileChunkStream {
        let buf = Box::new(unsafe { mem::uninitialized() });
        FileChunkStream { file, buf }
    }
}

impl Stream for FileChunkStream {
    type Item = Chunk;
    type Error = IoError;

    fn poll(&mut self) -> Poll<Option<Chunk>, IoError> {
        match self.file.poll_read(&mut self.buf[..]) {
            Ok(Async::Ready(0)) => Ok(Async::Ready(None)),
            Ok(Async::Ready(size)) => Ok(Async::Ready(Some(
                self.buf[..size].to_owned().into()))),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(e) => Err(e),
        }
    }
}

const BUF_SIZE: usize = 8 * 1024;

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
#[derive(Clone)]
pub struct Static<U = DefaultUpstream> {
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
    pub fn with_upstream<P: Into<PathBuf>>(root: P, upstream: U) -> Self {
        Self {
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
    pub fn new<P: Into<PathBuf>>(root: P) -> Self {
        Self::with_upstream(root, DefaultUpstream)
    }
}

impl<U> Service for Static<U>
        where U: Service<
            ReqBody = Body,
            ResBody = Body,
            Error = String,
            Future = ResponseFuture
        > {
    type ReqBody = Body;
    type ResBody = Body;
    type Error = String;
    type Future = ResponseFuture;

    fn call(&mut self, req: Request) -> ResponseFuture {
        // Handle only `GET`/`HEAD` and absolute paths.
        match req.method() {
            &Method::HEAD | &Method::GET => {},
            _ => return self.upstream.call(req),
        }

        if req.uri().scheme_part().is_some() || req.uri().host().is_some() {
            return self.upstream.call(req);
        }

        let requested_path = RequestedPath::new(&self.root, req.uri().path());

        let metadata = match fs::metadata(&requested_path.path) {
            Ok(meta) => meta,
            Err(e) => {
                return match e.kind() {
                    IoErrorKind::NotFound => {
                        self.upstream.call(req)
                    },
                    IoErrorKind::PermissionDenied => {
                        Box::new(future::result(
                            ResponseBuilder::new()
                                .status(StatusCode::FORBIDDEN)
                                .body(Body::empty())
                                .map_err(|e| e.to_string())
                        ))
                    },
                    _ => {
                        Box::new(future::err(e.to_string()))
                    },
                };
            },
        };

        // If the URL ends in a slash, serve the file directly.
        // Otherwise, redirect to the directory equivalent of the URL.
        if requested_path.should_redirect(&metadata, req.uri().path()) {
            // Append the trailing slash
            let mut target = req.uri().path().to_owned();
            target.push('/');
            if let Some(query) = req.uri().query() {
                target.push('?');
                target.push_str(query);
            }

            // Perform an HTTP 301 Redirect.
            return Box::new(future::result(
                ResponseBuilder::new()
                    .status(StatusCode::MOVED_PERMANENTLY)
                    .header(header::LOCATION, target.as_str())
                    .body(Body::empty())
                    .map_err(|e| e.to_string())
            ));
        }

        // Resolve the directory index, if necessary.
        let (path, metadata) = match requested_path.get_file(metadata) {
            None => return self.upstream.call(req),
            Some(val) => val,
        };

        // Check If-Modified-Since header.
        let modified: DateTime<LocalTz> = match metadata.modified() {
            Ok(time) => time.into(),
            Err(e) => return Box::new(future::err(e.to_string())),
        };

        let if_modified = req.headers().get(header::IF_MODIFIED_SINCE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| DateTime::parse_from_rfc2822(v).ok())
            .map(|v| v.with_timezone(&LocalTz));
        match if_modified {
            // Truncate before comparison, because the `Last-Modified` we serve
            // is also truncated through `DateTime::to_rfc2822`.
            Some(v) if modified.trunc_subsecs(0) <= v.trunc_subsecs(0) => {
                return Box::new(future::result(
                    ResponseBuilder::new()
                        .status(StatusCode::NOT_MODIFIED)
                        .body(Body::empty())
                        .map_err(|e| e.to_string())
                ));
            },
            _ => {},
        }

        // Build response headers.
        let mut res = ResponseBuilder::new();
        res.header(header::CONTENT_LENGTH, format!("{}", metadata.len()).as_str());
        res.header(header::LAST_MODIFIED, modified.to_rfc2822().as_str());
        res.header(header::ETAG, format!("W/\"{0:x}-{1:x}.{2:x}\"",
            metadata.len(), modified.timestamp(), modified.timestamp_subsec_nanos()).as_str());
        if self.cache_seconds != 0 {
            res.header(header::CACHE_CONTROL, format!("public, max-age={}", self.cache_seconds).as_str());
        }

        // Stream response body.
        match req.method() {
            &Method::HEAD => {
                Box::new(future::result(
                    res.body(Body::empty())
                        .map_err(|e| e.to_string())
                ))
            },
            &Method::GET => {
                Box::new(
                    File::open(path)
                        .map_err(|e| e.to_string())
                        .and_then(move |file| {
                            res.body(Body::wrap_stream(FileChunkStream::new(file)))
                                .map_err(|e| e.to_string())
                        })
                )
            },
            _ => unreachable!(),
        }
    }
}
