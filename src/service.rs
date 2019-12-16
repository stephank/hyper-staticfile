use crate::{resolve, ResponseBuilder};
use http::{Request, Response};
use hyper::{service::Service, Body};
use std::future::Future;
use std::io::Error as IoError;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

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
/// Note that using the trait currently involves an extra `Box`.
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
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Static {
            root,
            cache_headers: None,
        }
    }

    /// Add cache headers to responses for the given lifespan.
    pub fn cache_headers(&mut self, value: Option<u32>) -> &mut Self {
        self.cache_headers = value;
        self
    }

    /// Serve a request.
    pub async fn serve<B>(self, request: Request<B>) -> Result<Response<Body>, IoError> {
        let Self {
            root,
            cache_headers,
        } = self;
        resolve(root, &request).await.map(|result| {
            ResponseBuilder::new()
                .request(&request)
                .cache_headers(cache_headers)
                .build(result)
                .expect("unable to build response")
        })
    }
}

impl<B: Send + Sync + 'static> Service<Request<B>> for Static {
    type Response = Response<Body>;
    type Error = IoError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        Box::pin(self.clone().serve(request))
    }
}
