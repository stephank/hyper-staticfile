use ::{ResolveFuture, ResponseBuilder, resolve};
use futures::{Async::*, Future, Poll};
use http::{Request, Response};
use hyper::{Body, service::Service};
use std::io::Error;
use std::path::PathBuf;

/// Future returned by `Static::serve`.
pub struct StaticFuture<B> {
    /// Whether to send cache headers, and what lifespan to indicate.
    cache_headers: Option<u32>,
    /// Future for the `resolve` in progress.
    resolve_future: ResolveFuture,
    /// Request we're serving.
    request: Request<B>,
}

impl<B> Future for StaticFuture<B> {
    type Item = Response<Body>;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let result = try_ready!(self.resolve_future.poll());
        let response = ResponseBuilder::new()
            .cache_headers(self.cache_headers)
            .build(&self.request, result)
            .expect("unable to build response");
        Ok(Ready(response))
    }
}

/// High-level interface for serving static files.
///
/// This struct serves files from a single root path, which may be absolute or relative. The
/// request is mapped onto the filesystem by appending their URL path to the root path. If the
/// filesystem path corresponds to a regular file, the service will attempt to serve it. Otherwise,
/// if the path corresponds to a directory containing an `index.html`, the service will attempt to
/// serve that instead.
///
/// This struct allows direct access to its fields, but these fields are typically initialized by
/// the accessors, using the builder pattern. The fields are basically a bunch of settings that
/// determine the response details.
///
/// This struct also implements the `hyper::Service` trait, which simply wraps `Static::serve`.
#[derive(Clone)]
pub struct Static {
    /// The root directory path to serve files from.
    pub root: PathBuf,
    /// Whether to send cache headers, and what lifespan to indicate.
    pub cache_headers: Option<u32>,
}

impl Static {
    /// Create a new instance of `Static` with a given root path.
    ///
    /// If `Path::new("")` is given, files will be served from the current directory.
    pub fn new<P: Into<PathBuf>>(root: P) -> Self {
        let root = root.into();
        Static { root, cache_headers: None }
    }

    /// Add cache headers to responses for the given lifespan.
    pub fn cache_headers(&mut self, value: Option<u32>) -> &mut Self {
        self.cache_headers = value;
        self
    }

    /// Serve a request.
    pub fn serve<B>(&self, request: Request<B>) -> StaticFuture<B> {
        StaticFuture {
            cache_headers: self.cache_headers,
            resolve_future: resolve(&self.root, &request),
            request,
        }
    }
}

impl Service for Static {
    type ReqBody = Body;
    type ResBody = Body;
    type Error = Error;
    type Future = StaticFuture<Body>;

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        self.serve(request)
    }
}
