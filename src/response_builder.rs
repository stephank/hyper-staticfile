use crate::resolve::ResolveResult;
use crate::util::FileResponseBuilder;
use http::response::Builder as HttpResponseBuilder;
use http::{header, HeaderMap, Method, Request, Response, Result, StatusCode, Uri};
use hyper::Body;
use tokio::io::{AsyncRead, AsyncSeek};

/// Utility to build the default response for a `resolve` result.
///
/// This struct allows direct access to its fields, but these fields are typically initialized by
/// the accessors, using the builder pattern. The fields are basically a bunch of settings that
/// determine the response details.
#[derive(Clone, Debug, Default)]
pub struct ResponseBuilder<'a> {
    /// The request path.
    pub path: &'a str,
    /// The request query string.
    pub query: Option<&'a str>,
    /// Inner file response builder.
    pub file_response_builder: FileResponseBuilder,
}

impl<'a> ResponseBuilder<'a> {
    /// Create a new builder with a default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply parameters based on a request.
    pub fn request<B>(&mut self, req: &'a Request<B>) -> &mut Self {
        self.request_parts(req.method(), req.uri(), req.headers());
        self
    }

    /// Apply parameters based on request parts.
    pub fn request_parts(
        &mut self,
        method: &Method,
        uri: &'a Uri,
        headers: &'a HeaderMap,
    ) -> &mut Self {
        self.request_uri(uri);
        self.file_response_builder.request_parts(method, headers);
        self
    }

    /// Apply parameters based on a request URI.
    pub fn request_uri(&mut self, uri: &'a Uri) -> &mut Self {
        self.path(uri.path());
        self.query(uri.query());
        self
    }

    /// Add cache headers to responses for the given lifespan.
    pub fn cache_headers(&mut self, value: Option<u32>) -> &mut Self {
        self.file_response_builder.cache_headers(value);
        self
    }

    /// Set the request path.
    pub fn path(&mut self, value: &'a str) -> &mut Self {
        self.path = value;
        self
    }

    /// Set the request query string.
    pub fn query(&mut self, value: Option<&'a str>) -> &mut Self {
        self.query = value;
        self
    }

    /// Build a response for the given request and `resolve` result.
    ///
    /// This function may error if it response could not be constructed, but this should be a
    /// seldom occurrence.
    pub fn build<F>(&self, result: ResolveResult<F>) -> Result<Response<Body>>
    where
        F: AsyncRead + AsyncSeek + Send + Unpin + 'static,
    {
        match result {
            ResolveResult::MethodNotMatched => HttpResponseBuilder::new()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::empty()),
            ResolveResult::NotFound => HttpResponseBuilder::new()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty()),
            ResolveResult::PermissionDenied => HttpResponseBuilder::new()
                .status(StatusCode::FORBIDDEN)
                .body(Body::empty()),
            ResolveResult::IsDirectory => {
                let mut target = self.path.to_owned();
                target.push('/');
                if let Some(query) = self.query {
                    target.push('?');
                    target.push_str(query);
                }

                HttpResponseBuilder::new()
                    .status(StatusCode::MOVED_PERMANENTLY)
                    .header(header::LOCATION, target)
                    .body(Body::empty())
            }
            ResolveResult::Found(file) => self.file_response_builder.build(file),
        }
    }
}
