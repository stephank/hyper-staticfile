use chrono::{DateTime, SubsecRound, offset::Local as LocalTz};
use http::response::Builder as ResponseBuilder;
use http::{Method, Request, Response, Result, StatusCode, header};
use hyper::Body;
use std::fs::Metadata;
use super::FileChunkStream;
use tokio::fs::File;

/// Utility to build responses for serving a static file.
#[derive(Default)]
pub struct FileResponseBuilder {
    pub cache_headers: Option<u32>,
    pub is_head: bool,
    pub if_modified_since: Option<DateTime<LocalTz>>,
}

impl FileResponseBuilder {
    /// Create a new builder for the given request.
    pub fn from_request<B>(req: &Request<B>) -> Self {
        let mut builder = Self::default();
        builder.method(req.method());
        builder.if_modified_since_header(req.headers().get(header::IF_MODIFIED_SINCE));
        builder
    }

    /// Add cache headers to responses for the given lifespan.
    pub fn cache_headers(&mut self, value: Option<u32>) -> &mut Self {
        self.cache_headers = value;
        self
    }

    /// Build responses for the given request method.
    pub fn method(&mut self, value: &Method) -> &mut Self {
        self.is_head = *value == Method::HEAD;
        self
    }

    /// Build responses for the given `If-Modified-Since` request header value.
    pub fn if_modified_since_header(&mut self, value: Option<&header::HeaderValue>) -> &mut Self {
        self.if_modified_since = value
            .and_then(|v| v.to_str().ok())
            .and_then(|v| DateTime::parse_from_rfc2822(v).ok())
            .map(|v| v.with_timezone(&LocalTz));
        self
    }

    /// Build a response for the given file and metadata.
    pub fn build(&self, file: File, metadata: Metadata) -> Result<Response<Body>> {
        let mut res = ResponseBuilder::new();

        // Set `Last-Modified` and check `If-Modified-Since`.
        if let Ok(modified) = metadata.modified() {
            let modified: DateTime<LocalTz> = modified.into();

            match self.if_modified_since {
                // Truncate before comparison, because the `Last-Modified` we serve
                // is also truncated through `DateTime::to_rfc2822`.
                Some(v) if modified.trunc_subsecs(0) <= v.trunc_subsecs(0) => {
                    return ResponseBuilder::new()
                        .status(StatusCode::NOT_MODIFIED)
                        .body(Body::empty())
                },
                _ => {},
            }

            res.header(header::LAST_MODIFIED, modified.to_rfc2822().as_str());
            res.header(header::ETAG, format!("W/\"{0:x}-{1:x}.{2:x}\"",
                metadata.len(), modified.timestamp(), modified.timestamp_subsec_nanos()).as_str());
        }

        // Build remaining headers.
        res.header(header::CONTENT_LENGTH, format!("{}", metadata.len()).as_str());
        if let Some(seconds) = self.cache_headers {
            res.header(header::CACHE_CONTROL,
                format!("public, max-age={}", seconds).as_str());
        }

        // Stream the body.
        res.body(if self.is_head {
            Body::empty()
        } else {
            Body::wrap_stream(FileChunkStream::new(file))
        })
    }
}
