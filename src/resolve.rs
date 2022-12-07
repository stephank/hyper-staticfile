use crate::util::{open_with_metadata, RequestedPath};
use http::{header, HeaderValue, Method, Request};
use mime_guess::{Mime, MimeGuess};
use std::fs::Metadata;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::ops::BitAnd;
use std::path::PathBuf;
use tokio::fs::File;

/// This struct resolves files from a single root path, which may be absolute or relative. A
/// request is mapped onto the filesystem by appending its URL path to the root path. If the
/// filesystem path corresponds to a regular file, the service will attempt to serve it. Otherwise,
/// if the path corresponds to a directory containing an `index.html`, the service will attempt to
/// serve that instead.
#[derive(Clone)]
pub struct Resolver {
    /// The root directory path to resolve files against.
    pub root: PathBuf,

    /// Encodings the client is allowed to request with `Accept-Encoding`.
    ///
    /// This only supports pre-encoded files, that exist adjacent to the original file, but with an
    /// additional `.br` or `.gz` suffix (after the original extension).
    ///
    /// Typically initialized with `AcceptEncoding::all()` or `AcceptEncoding::none()`.
    pub allowed_encodings: AcceptEncoding,
}

/// The result of `resolve`.
///
/// Covers all the possible 'normal' scenarios encountered when serving static files.
#[derive(Debug)]
pub enum ResolveResult {
    /// The request was not `GET` or `HEAD` request,
    MethodNotMatched,
    /// The requested file does not exist.
    NotFound,
    /// The requested file could not be accessed.
    PermissionDenied,
    /// A directory was requested as a file.
    IsDirectory,
    /// The requested file was found.
    Found(File, Metadata, Mime),
    /// A pre-encoded version of the requested file was found.
    FoundEncoded(File, Metadata, Mime, Encoding),
}

/// Some IO errors are expected when serving files, and mapped to a regular result here.
fn map_open_err(err: IoError) -> Result<ResolveResult, IoError> {
    match err.kind() {
        IoErrorKind::NotFound => Ok(ResolveResult::NotFound),
        IoErrorKind::PermissionDenied => Ok(ResolveResult::PermissionDenied),
        _ => Err(err),
    }
}

impl Resolver {
    /// Create a resolver.
    ///
    /// Short-hand that sets `allowed_encodings` to none.
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            allowed_encodings: AcceptEncoding::none(),
        }
    }

    /// Resolve the request by trying to find the file in the root.
    ///
    /// The returned future may error for unexpected IO errors, passing on the `std::io::Error`.
    /// Certain expected IO errors are handled, though, and simply reflected in the result. These are
    /// `NotFound` and `PermissionDenied`.
    pub async fn resolve_request<B>(&self, req: &Request<B>) -> Result<ResolveResult, IoError> {
        // Handle only `GET`/`HEAD` and absolute paths.
        match *req.method() {
            Method::HEAD | Method::GET => {}
            _ => {
                return Ok(ResolveResult::MethodNotMatched);
            }
        }

        // Parse `Accept-Encoding` header.
        let accept_encoding = self.allowed_encodings
            & req
                .headers()
                .get(header::ACCEPT_ENCODING)
                .map(AcceptEncoding::from_header_value)
                .unwrap_or(AcceptEncoding::none());

        self.resolve_path(req.uri().path(), accept_encoding).await
    }

    /// Resolve the request path by trying to find the file in the given root.
    ///
    /// The returned future may error for unexpected IO errors, passing on the `std::io::Error`.
    /// Certain expected IO errors are handled, though, and simply reflected in the result. These are
    /// `NotFound` and `PermissionDenied`.
    ///
    /// Note that, unlike `resolve_request`, it is up to the caller to check the request method and
    /// optionally the 'Accept-Encoding' header.
    pub async fn resolve_path(
        &self,
        request_path: &str,
        accept_encoding: AcceptEncoding,
    ) -> Result<ResolveResult, IoError> {
        let RequestedPath {
            mut full_path,
            is_dir_request,
        } = RequestedPath::resolve(self.root.to_owned(), request_path);

        let (file, metadata) = match open_with_metadata(&full_path).await {
            Ok(pair) => pair,
            Err(err) => return map_open_err(err),
        };

        // The resolved `full_path` doesn't contain the trailing slash anymore, so we may
        // have opened a file for a directory request, which we treat as 'not found'.
        if is_dir_request && !metadata.is_dir() {
            return Ok(ResolveResult::NotFound);
        }

        // We may have opened a directory for a file request, in which case we redirect.
        if !is_dir_request && metadata.is_dir() {
            return Ok(ResolveResult::IsDirectory);
        }

        // If not a directory, serve this file.
        if !is_dir_request {
            return Self::resolve_final(file, metadata, full_path, accept_encoding).await;
        }

        // Resolve the directory index.
        full_path.push("index.html");
        let (file, metadata) = match open_with_metadata(&full_path).await {
            Ok(pair) => pair,
            Err(err) => return map_open_err(err),
        };

        // The directory index cannot itself be a directory.
        if metadata.is_dir() {
            return Ok(ResolveResult::NotFound);
        }

        // Serve this file.
        Self::resolve_final(file, metadata, full_path, accept_encoding).await
    }

    // Found a file, perform final resolution steps.
    async fn resolve_final(
        file: File,
        metadata: Metadata,
        full_path: PathBuf,
        accept_encoding: AcceptEncoding,
    ) -> Result<ResolveResult, IoError> {
        // Determine MIME-type. This needs to happen before we resolve a pre-encoded file.
        let mime = MimeGuess::from_path(&full_path).first_or_octet_stream();

        // Resolve pre-encoded files.
        if accept_encoding.br {
            let mut br_path = full_path.clone().into_os_string();
            br_path.push(".br");
            if let Ok((enc_file, enc_metadata)) = open_with_metadata(&br_path).await {
                return Ok(ResolveResult::FoundEncoded(
                    enc_file,
                    enc_metadata,
                    mime,
                    Encoding::Br,
                ));
            }
        }
        if accept_encoding.gzip {
            let mut gzip_path = full_path.into_os_string();
            gzip_path.push(".gz");
            if let Ok((enc_file, enc_metadata)) = open_with_metadata(&gzip_path).await {
                return Ok(ResolveResult::FoundEncoded(
                    enc_file,
                    enc_metadata,
                    mime,
                    Encoding::Gzip,
                ));
            }
        }

        // No pre-encoded file found, serve the original.
        Ok(ResolveResult::Found(file, metadata, mime))
    }
}

/// Type of response encoding.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Encoding {
    /// Response body is encoded with gzip.
    Gzip,
    /// Response body is encoded with brotli.
    Br,
}

impl Encoding {
    /// Create a `HeaderValue` for this encoding.
    pub fn to_header_value(&self) -> HeaderValue {
        HeaderValue::from_static(match self {
            Encoding::Gzip => "gzip",
            Encoding::Br => "br",
        })
    }
}

/// Flags for which encodings to resolve.
#[derive(Debug, Copy, Clone)]
pub struct AcceptEncoding {
    /// Look for `.gz` files.
    pub gzip: bool,
    /// Look for `.br` files.
    pub br: bool,
}

impl AcceptEncoding {
    /// Return an `AcceptEncoding` with all flags set.
    pub const fn all() -> Self {
        Self {
            gzip: true,
            br: true,
        }
    }

    /// Return an `AcceptEncoding` with no flags set.
    pub const fn none() -> Self {
        Self {
            gzip: false,
            br: false,
        }
    }

    /// Fill an `AcceptEncoding` struct from a header value.
    pub fn from_header_value(value: &HeaderValue) -> Self {
        let mut res = Self::none();
        if let Ok(value) = value.to_str() {
            for enc in value.split(",") {
                // TODO: Handle weights (q=)
                match enc.split(";").next().unwrap().trim() {
                    "gzip" => res.gzip = true,
                    "br" => res.br = true,
                    _ => {}
                }
            }
        }
        res
    }
}

impl BitAnd for AcceptEncoding {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self {
            gzip: self.gzip && rhs.gzip,
            br: self.br && rhs.br,
        }
    }
}
