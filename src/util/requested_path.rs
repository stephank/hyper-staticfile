use std::path::{Component, Path, PathBuf};

#[inline]
fn decode_percents(string: &str) -> String {
    percent_encoding::percent_decode_str(string)
        .decode_utf8_lossy()
        .into_owned()
}

fn sanitize_path(path: &Path) -> PathBuf {
    path.components()
        .fold(PathBuf::new(), |mut result, p| match p {
            Component::Normal(x) => {
                // Parse again to prevent a malicious component containing
                // a Windows drive letter, e.g.: `/anypath/c:/windows/win.ini`
                if Path::new(&x)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)))
                {
                    result.push(x);
                }
                result
            }
            Component::ParentDir => {
                result.pop();
                result
            }
            _ => result,
        })
}

/// Processed request path.
pub struct RequestedPath {
    /// Sanitized path of the request.
    pub sanitized: PathBuf,
    /// Whether a directory was requested. (The input ended with a slash.)
    pub is_dir_request: bool,
}

impl RequestedPath {
    /// Process a request path.
    pub fn resolve(request_path: &str) -> Self {
        let is_dir_request = request_path.as_bytes().last() == Some(&b'/');
        let request_path = PathBuf::from(decode_percents(request_path));
        RequestedPath {
            sanitized: sanitize_path(&request_path),
            is_dir_request,
        }
    }
}
