use futures::{Async::*, Future, Poll};
use http::{Method, Request};
use mime_guess::{Mime, MimeGuess};
use std::convert::AsRef;
use std::fs::Metadata;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use util::{open_with_metadata, OpenWithMetadataFuture, RequestedPath};

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

/// State of `resolve` as it progresses.
enum ResolveState {
    /// Immediate result for method not matched.
    MethodNotMatched,
    /// Immediate result for route not matched.
    UriNotMatched,
    /// Wait for the file to open.
    WaitOpen(OpenWithMetadataFuture),
    /// Wait for the directory index file to open.
    WaitOpenIndex(OpenWithMetadataFuture),
}

/// Some IO errors are expected when serving files, and mapped to a regular result here.
fn map_open_err(err: Error) -> Poll<ResolveResult, Error> {
    match err.kind() {
        ErrorKind::NotFound => Ok(Ready(ResolveResult::NotFound)),
        ErrorKind::PermissionDenied => Ok(Ready(ResolveResult::PermissionDenied)),
        _ => Err(err),
    }
}

/// Future returned by `resolve`.
pub struct ResolveFuture {
    /// Resolved filesystem path. An option, because we take ownership later.
    full_path: Option<PathBuf>,
    /// Whether this is a directory request. (Request path ends with a slash.)
    is_dir_request: bool,
    /// Current state of this future.
    state: ResolveState,
}

impl Future for ResolveFuture {
    type Item = ResolveResult;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            self.state = match self.state {
                ResolveState::MethodNotMatched => {
                    return Ok(Ready(ResolveResult::MethodNotMatched));
                }
                ResolveState::UriNotMatched => {
                    return Ok(Ready(ResolveResult::UriNotMatched));
                }
                ResolveState::WaitOpen(ref mut future) => {
                    let (file, metadata) = match future.poll() {
                        Ok(Ready(pair)) => pair,
                        Ok(NotReady) => return Ok(NotReady),
                        Err(err) => return map_open_err(err),
                    };

                    // The resolved `full_path` doesn't contain the trailing slash anymore, so we may
                    // have opened a file for a directory request, which we treat as 'not found'.
                    if self.is_dir_request && !metadata.is_dir() {
                        return Ok(Ready(ResolveResult::NotFound));
                    }

                    // We may have opened a directory for a file request, in which case we redirect.
                    if !self.is_dir_request && metadata.is_dir() {
                        return Ok(Ready(ResolveResult::IsDirectory));
                    }

                    // If not a directory, serve this file.
                    if !self.is_dir_request {
                        let mime =
                            MimeGuess::from_path(self.full_path.as_ref().expect("invalid state"))
                                .first_or_octet_stream();
                        return Ok(Ready(ResolveResult::Found(file, metadata, mime)));
                    }

                    // Resolve the directory index.
                    let full_path = self.full_path.as_mut().expect("invalid state");
                    full_path.push("index.html");
                    ResolveState::WaitOpenIndex(open_with_metadata(full_path.to_path_buf()))
                }
                ResolveState::WaitOpenIndex(ref mut future) => {
                    let (file, metadata) = match future.poll() {
                        Ok(Ready(pair)) => pair,
                        Ok(NotReady) => return Ok(NotReady),
                        Err(err) => return map_open_err(err),
                    };

                    // The directory index cannot itself be a directory.
                    if metadata.is_dir() {
                        return Ok(Ready(ResolveResult::NotFound));
                    }

                    // Serve this file.
                    let mime =
                        MimeGuess::from_path(self.full_path.as_ref().expect("invalid state"))
                            .first_or_octet_stream();
                    return Ok(Ready(ResolveResult::Found(file, metadata, mime)));
                }
            }
        }
    }
}

/// Resolve the request by trying to find the file in the given root.
///
/// This root may be absolute or relative. The request is mapped onto the filesystem by appending
/// their URL path to the root path. If the filesystem path corresponds to a regular file, the
/// service will attempt to serve it. Otherwise, if the path corresponds to a directory containing
/// an `index.html`, the service will attempt to serve that instead.
///
/// The returned future may error for unexpected IO errors, passing on the `std::io::Error`.
/// Certain expected IO errors are handled, though, and simply reflected in the result. These are
/// `NotFound` and `PermissionDenied`.
pub fn resolve<B, P: AsRef<Path>>(root: P, req: &Request<B>) -> ResolveFuture {
    // Handle only `GET`/`HEAD` and absolute paths.
    match *req.method() {
        Method::HEAD | Method::GET => {}
        _ => {
            return ResolveFuture {
                full_path: None,
                is_dir_request: false,
                state: ResolveState::MethodNotMatched,
            };
        }
    }

    // Handle only simple path requests.
    if req.uri().scheme_part().is_some() || req.uri().host().is_some() {
        return ResolveFuture {
            full_path: None,
            is_dir_request: false,
            state: ResolveState::UriNotMatched,
        };
    }

    let RequestedPath {
        full_path,
        is_dir_request,
    } = RequestedPath::resolve(root.as_ref(), req.uri().path());

    let state = ResolveState::WaitOpen(open_with_metadata(full_path.clone()));
    let full_path = Some(full_path);
    ResolveFuture {
        full_path,
        is_dir_request,
        state,
    }
}
