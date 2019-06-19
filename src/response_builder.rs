use http::response::Builder as HttpResponseBuilder;
use http::{header, Request, Response, Result, StatusCode};
use hyper::Body;
use resolve::ResolveResult;
use util::FileResponseBuilder;

/// Utility to build the default response for a `resolve` result.
///
/// This struct allows direct access to its fields, but these fields are typically initialized by
/// the accessors, using the builder pattern. The fields are basically a bunch of settings that
/// determine the response details.
#[derive(Clone, Debug, Default)]
pub struct ResponseBuilder {
    /// Whether to send cache headers, and what lifespan to indicate.
    pub cache_headers: Option<u32>,
}

impl ResponseBuilder {
    /// Create a new builder with a default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add cache headers to responses for the given lifespan.
    pub fn cache_headers(&mut self, value: Option<u32>) -> &mut Self {
        self.cache_headers = value;
        self
    }

    /// Build a response for the given request and `resolve` result.
    ///
    /// This function may error if it response could not be constructed, but this should be a
    /// seldom occurrence.
    pub fn build<B>(&self, req: &Request<B>, result: ResolveResult) -> Result<Response<Body>> {
        match result {
            ResolveResult::MethodNotMatched => HttpResponseBuilder::new()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::empty()),
            ResolveResult::UriNotMatched | ResolveResult::NotFound => HttpResponseBuilder::new()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty()),
            ResolveResult::PermissionDenied => HttpResponseBuilder::new()
                .status(StatusCode::FORBIDDEN)
                .body(Body::empty()),
            ResolveResult::IsDirectory => {
                let mut target = req.uri().path().to_owned();
                target.push('/');
                if let Some(query) = req.uri().query() {
                    target.push('?');
                    target.push_str(query);
                }

                HttpResponseBuilder::new()
                    .status(StatusCode::MOVED_PERMANENTLY)
                    .header(header::LOCATION, target.as_str())
                    .body(Body::empty())
            }
            ResolveResult::Found(file, metadata, mime) => FileResponseBuilder::from_request(req)
                .cache_headers(self.cache_headers)
                .build(file, metadata)
                .map(|mut r| {
                    let header_val =
                        header::HeaderValue::from_str(&mime.to_string()).expect("Invalid mimetype");
                    r.headers_mut().insert(header::CONTENT_TYPE, header_val);
                    r
                }),
        }
    }
}
