use std::{
    io::{Error as IoError, ErrorKind as IoErrorKind},
    ops::BitAnd,
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};

use http::{header, HeaderValue, Method, Request};
use mime_guess::MimeGuess;
use tokio::fs::File;

use crate::{
    util::RequestedPath,
    vfs::{FileOpener, FileWithMetadata, TokioFileOpener},
};

/// Struct containing all the required data to serve a file.
#[derive(Debug)]
pub struct ResolvedFile<F = File> {
    /// Open file handle.
    pub handle: F,
    /// The resolved and sanitized path to the file.
    /// For directory indexes, this includes `index.html`.
    /// For pre-encoded files, this will include the compressed extension. (`.gz` or `.br`)
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// Last modification time.
    pub modified: Option<SystemTime>,
    /// MIME type / 'Content-Type' value.
    pub content_type: Option<String>,
    /// 'Content-Encoding' value.
    pub encoding: Option<Encoding>,
}

impl<F> ResolvedFile<F> {
    fn new(
        file: FileWithMetadata<F>,
        path: PathBuf,
        content_type: Option<String>,
        encoding: Option<Encoding>,
    ) -> Self {
        Self {
            handle: file.handle,
            path,
            size: file.size,
            modified: file.modified,
            content_type,
            encoding,
        }
    }
}

/// Resolves request paths to files.
///
/// This struct resolves files based on the request path. The path is first sanitized, then mapped
/// to a file on the filesystem. If the path corresponds to a directory, it will try to look for a
/// directory index.
///
/// Cloning this struct is a cheap operation.
pub struct Resolver<O = TokioFileOpener> {
    /// The (virtual) filesystem used to open files.
    pub opener: Arc<O>,

    /// Encodings the client is allowed to request with `Accept-Encoding`.
    ///
    /// This only supports pre-encoded files, that exist adjacent to the original file, but with an
    /// additional `.br` or `.gz` suffix (after the original extension).
    ///
    /// Typically initialized with `AcceptEncoding::all()` or `AcceptEncoding::none()`.
    pub allowed_encodings: AcceptEncoding,
}

/// The result of `Resolver` methods.
///
/// Covers all the possible 'normal' scenarios encountered when serving static files.
#[derive(Debug)]
pub enum ResolveResult<F = File> {
    /// The request was not `GET` or `HEAD` request,
    MethodNotMatched,
    /// The requested file does not exist.
    NotFound,
    /// The requested file could not be accessed.
    PermissionDenied,
    /// A directory was requested as a file.
    IsDirectory {
        /// Path to redirect to.
        redirect_to: String,
    },
    /// The requested file was found.
    Found(ResolvedFile<F>),
}

/// Some IO errors are expected when serving files, and mapped to a regular result here.
fn map_open_err<F>(err: IoError) -> Result<ResolveResult<F>, IoError> {
    match err.kind() {
        IoErrorKind::NotFound => Ok(ResolveResult::NotFound),
        IoErrorKind::PermissionDenied => Ok(ResolveResult::PermissionDenied),
        _ => Err(err),
    }
}

impl Resolver<TokioFileOpener> {
    /// Create a resolver that resolves files inside a root directory on the regular filesystem.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_opener(TokioFileOpener::new(root))
    }
}

impl<O: FileOpener> Resolver<O> {
    /// Create a resolver with a custom file opener.
    pub fn with_opener(opener: O) -> Self {
        Self {
            opener: Arc::new(opener),
            allowed_encodings: AcceptEncoding::none(),
        }
    }

    /// Resolve the request by trying to find the file in the root.
    ///
    /// The returned future may error for unexpected IO errors, passing on the `std::io::Error`.
    /// Certain expected IO errors are handled, though, and simply reflected in the result. These are
    /// `NotFound` and `PermissionDenied`.
    pub async fn resolve_request<B>(
        &self,
        req: &Request<B>,
    ) -> Result<ResolveResult<O::File>, IoError> {
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
    ) -> Result<ResolveResult<O::File>, IoError> {
        // Sanitize input path.
        let RequestedPath {
            sanitized: mut path,
            is_dir_request,
        } = RequestedPath::resolve(request_path);

        // Try to open the file.
        let file = match self.opener.open(&path).await {
            Ok(pair) => pair,
            Err(err) => return map_open_err(err),
        };

        // The resolved path doesn't contain the trailing slash anymore, so we may
        // have opened a file for a directory request, which we treat as 'not found'.
        if is_dir_request && !file.is_dir {
            return Ok(ResolveResult::NotFound);
        }

        // We may have opened a directory for a file request, in which case we redirect.
        if !is_dir_request && file.is_dir {
            // Build the redirect path. On Windows, we can't just append the entire path, because
            // it contains Windows path separators. Instead, append each component separately.
            let mut target = String::with_capacity(path.as_os_str().len() + 2);
            target.push('/');
            for component in path.components() {
                target.push_str(&component.as_os_str().to_string_lossy());
                target.push('/');
            }

            return Ok(ResolveResult::IsDirectory {
                redirect_to: target,
            });
        }

        // If not a directory, serve this file.
        if !is_dir_request {
            return self.resolve_final(file, path, accept_encoding).await;
        }

        // Resolve the directory index.
        path.push("index.html");
        let file = match self.opener.open(&path).await {
            Ok(pair) => pair,
            Err(err) => return map_open_err(err),
        };

        // The directory index cannot itself be a directory.
        if file.is_dir {
            return Ok(ResolveResult::NotFound);
        }

        // Serve this file.
        self.resolve_final(file, path, accept_encoding).await
    }

    // Found a file, perform final resolution steps.
    async fn resolve_final(
        &self,
        file: FileWithMetadata<O::File>,
        path: PathBuf,
        accept_encoding: AcceptEncoding,
    ) -> Result<ResolveResult<O::File>, IoError> {
        // Determine MIME-type. This needs to happen before we resolve a pre-encoded file.
        let mime = MimeGuess::from_path(&path)
            .first()
            .map(|mime| mime.to_string());

        // Resolve pre-encoded files.
        if accept_encoding.br {
            let mut br_path = path.clone().into_os_string();
            br_path.push(".br");
            if let Ok(file) = self.opener.open(br_path.as_ref()).await {
                return Ok(ResolveResult::Found(ResolvedFile::new(
                    file,
                    br_path.into(),
                    mime,
                    Some(Encoding::Br),
                )));
            }
        }
        if accept_encoding.gzip {
            let mut gzip_path = path.clone().into_os_string();
            gzip_path.push(".gz");
            if let Ok(file) = self.opener.open(gzip_path.as_ref()).await {
                return Ok(ResolveResult::Found(ResolvedFile::new(
                    file,
                    gzip_path.into(),
                    mime,
                    Some(Encoding::Gzip),
                )));
            }
        }

        // No pre-encoded file found, serve the original.
        Ok(ResolveResult::Found(ResolvedFile::new(
            file, path, mime, None,
        )))
    }
}

impl<O> Clone for Resolver<O> {
    fn clone(&self) -> Self {
        Self {
            opener: self.opener.clone(),
            allowed_encodings: self.allowed_encodings,
        }
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
            for enc in value.split(',') {
                // TODO: Handle weights (q=)
                match enc.split(';').next().unwrap().trim() {
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
