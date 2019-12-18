use crate::util::{open_with_metadata, RequestedPath};
use http::{Method, Request};
use mime_guess::{Mime, MimeGuess};
use std::fs::{File, Metadata};
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::path::PathBuf;

/// The result of `resolve`.
///
/// Covers all the possible 'normal' scenarios encountered when serving static files.
#[derive(Debug)]
pub enum ResolveResult {
    /// The request was not `GET` or `HEAD` request,
    MethodNotMatched,
    /// The request URI was not just a path.
    UriNotMatched,
    /// The requested file does not exist.
    NotFound,
    /// The requested file could not be accessed.
    PermissionDenied,
    /// A directory was requested as a file.
    IsDirectory,
    /// The requested file was found.
    Found(File, Metadata, Mime),
}

/// Some IO errors are expected when serving files, and mapped to a regular result here.
fn map_open_err(err: IoError) -> Result<ResolveResult, IoError> {
    match err.kind() {
        IoErrorKind::NotFound => Ok(ResolveResult::NotFound),
        IoErrorKind::PermissionDenied => Ok(ResolveResult::PermissionDenied),
        _ => Err(err),
    }
}

/// Resolve the request by trying to find the file in the given root.
///
/// This root may be absolute or relative. The request is mapped onto the filesystem by appending
/// its URL path to the root path. If the filesystem path corresponds to a regular file, the
/// service will attempt to serve it. Otherwise, if the path corresponds to a directory containing
/// an `index.html`, the service will attempt to serve that instead.
///
/// The returned future may error for unexpected IO errors, passing on the `std::io::Error`.
/// Certain expected IO errors are handled, though, and simply reflected in the result. These are
/// `NotFound` and `PermissionDenied`.
pub async fn resolve<B>(
    root: impl Into<PathBuf>,
    req: &Request<B>,
) -> Result<ResolveResult, IoError> {
    // Handle only `GET`/`HEAD` and absolute paths.
    match *req.method() {
        Method::HEAD | Method::GET => {}
        _ => {
            return Ok(ResolveResult::MethodNotMatched);
        }
    }

    // Handle only simple path requests.
    if req.uri().scheme_str().is_some() || req.uri().host().is_some() {
        return Ok(ResolveResult::UriNotMatched);
    }

    resolve_path(root, req.uri().path()).await
}

/// Resolve the request path by trying to find the file in the given root.
///
/// This root may be absolute or relative. The request path is mapped onto the filesystem by
/// appending it to the root path. If the filesystem path corresponds to a regular file, the
/// service will attempt to serve it. Otherwise, if the path corresponds to a directory containing
/// an `index.html`, the service will attempt to serve that instead.
///
/// The returned future may error for unexpected IO errors, passing on the `std::io::Error`.
/// Certain expected IO errors are handled, though, and simply reflected in the result. These are
/// `NotFound` and `PermissionDenied`.
///
/// Note that, unlike `resolve`, it is up to the caller to check the request method.
pub async fn resolve_path(
    root: impl Into<PathBuf>,
    request_path: &str,
) -> Result<ResolveResult, IoError> {
    let RequestedPath {
        mut full_path,
        is_dir_request,
    } = RequestedPath::resolve(root, request_path);

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
        let mime = MimeGuess::from_path(&full_path).first_or_octet_stream();
        return Ok(ResolveResult::Found(file, metadata, mime));
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
    let mime = MimeGuess::from_path(full_path).first_or_octet_stream();
    Ok(ResolveResult::Found(file, metadata, mime))
}
