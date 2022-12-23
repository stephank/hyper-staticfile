use http::{
    header, response::Builder as HttpResponseBuilder, HeaderMap, Method, Request, Response, Result,
    StatusCode, Uri,
};
use hyper::Body;

use crate::{
    resolve::ResolveResult,
    util::{FileResponseBuilder, RequestedPath},
    vfs::IntoFileAccess,
};

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
    pub fn build<F: IntoFileAccess>(&self, result: ResolveResult<F>) -> Result<Response<Body>> {
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
                // NOTE: We are doing an origin-relative redirect, but need to use the sanitized
                // path in order to prevent a malicious redirect to `//foo` (schema-relative).
                // With the current API, we have no other option here than to do sanitization
                // again, but a future version may reuse the earlier sanitization result.
                let resolved = RequestedPath::resolve(self.path);

                let mut target_len = resolved.sanitized.as_os_str().len() + 2;
                if let Some(ref query) = self.query {
                    target_len += query.len() + 1;
                }

                let mut target = String::with_capacity(target_len);
                target.push('/');
                // On Windows, we can't just append the entire path, because it contains Windows
                // path separators. Append per-component instead.
                for component in resolved.sanitized.components() {
                    target.push_str(&component.as_os_str().to_string_lossy());
                    target.push('/');
                }

                // Preserve any query string from the original request.
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
