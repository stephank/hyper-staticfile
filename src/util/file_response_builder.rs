use super::FileBytesStream;
use chrono::{offset::Local as LocalTz, DateTime, SubsecRound};
use http::response::Builder as ResponseBuilder;
use http::{header, HeaderMap, Method, Request, Response, Result, StatusCode};
use hyper::Body;
use std::fs::Metadata;
use std::time::{Duration, UNIX_EPOCH};
use tokio::fs::File;

/// Minimum duration since Unix epoch we accept for file modification time.
///
/// This is intended to discard invalid times, specifically:
///  - Zero values on any Unix system.
///  - 'Epoch + 1' on NixOS.
const MIN_VALID_MTIME: Duration = Duration::from_secs(2);

/// Utility to build responses for serving a `tokio::fs::File`.
///
/// This struct allows direct access to its fields, but these fields are typically initialized by
/// the accessors, using the builder pattern. The fields are basically a bunch of settings that
/// determine the response details.
#[derive(Clone, Debug, Default)]
pub struct FileResponseBuilder {
    /// Whether to send cache headers, and what lifespan to indicate.
    pub cache_headers: Option<u32>,
    /// Whether this is a `HEAD` request, with no response body.
    pub is_head: bool,
    /// The parsed value of the `If-Modified-Since` request header.
    pub if_modified_since: Option<DateTime<LocalTz>>,
}

impl FileResponseBuilder {
    /// Create a new builder with a default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply parameters based on a request.
    pub fn request<B>(&mut self, req: &Request<B>) -> &mut Self {
        self.request_parts(req.method(), req.headers())
    }

    /// Apply parameters based on request parts.
    pub fn request_parts(&mut self, method: &Method, headers: &HeaderMap) -> &mut Self {
        self.request_method(method);
        self.request_headers(headers);
        self
    }

    /// Apply parameters based on a request method.
    pub fn request_method(&mut self, method: &Method) -> &mut Self {
        self.is_head = *method == Method::HEAD;
        self
    }

    /// Apply parameters based on request headers.
    pub fn request_headers(&mut self, headers: &HeaderMap) -> &mut Self {
        self.if_modified_since_header(headers.get(header::IF_MODIFIED_SINCE));
        self
    }

    /// Add cache headers to responses for the given lifespan.
    pub fn cache_headers(&mut self, value: Option<u32>) -> &mut Self {
        self.cache_headers = value;
        self
    }

    /// Set whether this is a `HEAD` request, with no response body.
    pub fn is_head(&mut self, value: bool) -> &mut Self {
        self.is_head = value;
        self
    }

    /// Build responses for the given `If-Modified-Since` date-time.
    pub fn if_modified_since(&mut self, value: Option<DateTime<LocalTz>>) -> &mut Self {
        self.if_modified_since = value;
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
        let modified = metadata.modified().ok().filter(|v| {
            v.duration_since(UNIX_EPOCH)
                .ok()
                .filter(|v| v >= &MIN_VALID_MTIME)
                .is_some()
        });
        if let Some(modified) = modified {
            let modified: DateTime<LocalTz> = modified.into();

            match self.if_modified_since {
                // Truncate before comparison, because the `Last-Modified` we serve
                // is also truncated through `DateTime::to_rfc2822`.
                Some(v) if modified.trunc_subsecs(0) <= v.trunc_subsecs(0) => {
                    return ResponseBuilder::new()
                        .status(StatusCode::NOT_MODIFIED)
                        .body(Body::empty())
                }
                _ => {}
            }

            res = res
                .header(header::LAST_MODIFIED, modified.to_rfc2822().as_str())
                .header(
                    header::ETAG,
                    format!(
                        "W/\"{0:x}-{1:x}.{2:x}\"",
                        metadata.len(),
                        modified.timestamp(),
                        modified.timestamp_subsec_nanos()
                    )
                    .as_str(),
                );
        }

        // Build remaining headers.
        res = res.header(
            header::CONTENT_LENGTH,
            format!("{}", metadata.len()).as_str(),
        );
        if let Some(seconds) = self.cache_headers {
            res = res.header(
                header::CACHE_CONTROL,
                format!("public, max-age={}", seconds).as_str(),
            );
        }

        // Stream the body.
        res.body(if self.is_head {
            Body::empty()
        } else {
            FileBytesStream::new(file).into_body()
        })
    }
}
