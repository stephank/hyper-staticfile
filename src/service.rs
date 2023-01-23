use std::{future::Future, io::Error as IoError, path::PathBuf, pin::Pin};

use http::{Request, Response};
use hyper::service::Service;

use crate::{
    vfs::{FileOpener, IntoFileAccess, TokioFileOpener},
    AcceptEncoding, Body, Resolver, ResponseBuilder,
};

/// High-level interface for serving static files.
///
/// This services serves files based on the request path. The path is first sanitized, then mapped
/// to a file on the filesystem. If the path corresponds to a directory, it will try to look for a
/// directory index.
///
/// This struct allows direct access to its fields, but these fields are typically initialized by
/// the accessors, using the builder pattern. The fields are basically a bunch of settings that
/// determine the response details.
///
/// This struct also implements the `hyper::Service` trait, which simply wraps `Static::serve`.
/// Note that using the trait currently involves an extra `Box`.
///
/// Cloning this struct is a cheap operation.
pub struct Static<O = TokioFileOpener> {
    /// The resolver instance used to open files.
    pub resolver: Resolver<O>,
    /// Whether to send cache headers, and what lifespan to indicate.
    pub cache_headers: Option<u32>,
}

impl Static<TokioFileOpener> {
    /// Create a new instance of `Static` with a given root path.
    ///
    /// The path may be absolute or relative. If `Path::new("")` is used, files will be served from
    /// the current directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            resolver: Resolver::new(root),
            cache_headers: None,
        }
    }
}

impl<O: FileOpener> Static<O> {
    /// Create a new instance of `Static` with the given root directory.
    pub fn with_opener(opener: O) -> Self {
        Self {
            resolver: Resolver::with_opener(opener),
            cache_headers: None,
        }
    }

    /// Add cache headers to responses for the given lifespan.
    pub fn cache_headers(&mut self, value: Option<u32>) -> &mut Self {
        self.cache_headers = value;
        self
    }

    /// Set the encodings the client is allowed to request via the `Accept-Encoding` header.
    pub fn allowed_encodings(&mut self, allowed_encodings: AcceptEncoding) -> &mut Self {
        self.resolver.allowed_encodings = allowed_encodings;
        self
    }

    /// Serve a request.
    pub async fn serve<B>(
        self,
        request: Request<B>,
    ) -> Result<Response<Body<<O::File as IntoFileAccess>::Output>>, IoError> {
        let Self {
            resolver,
            cache_headers,
        } = self;
        resolver.resolve_request(&request).await.map(|result| {
            ResponseBuilder::new()
                .request(&request)
                .cache_headers(cache_headers)
                .build(result)
                .expect("unable to build response")
        })
    }
}

impl<O> Clone for Static<O> {
    fn clone(&self) -> Self {
        Self {
            resolver: self.resolver.clone(),
            cache_headers: self.cache_headers,
        }
    }
}

impl<O, B> Service<Request<B>> for Static<O>
where
    O: FileOpener,
    B: Send + Sync + 'static,
{
    type Response = Response<Body<<O::File as IntoFileAccess>::Output>>;
    type Error = IoError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&mut self, request: Request<B>) -> Self::Future {
        Box::pin(self.clone().serve(request))
    }
}
