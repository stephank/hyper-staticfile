use std::path::{Component, Path, PathBuf};
use url::percent_encoding::percent_decode;

#[inline]
fn decode_percents(string: &str) -> String {
    percent_decode(string.as_bytes())
        .decode_utf8()
        .unwrap()
        .into_owned()
}

fn normalize_path(path: &Path) -> PathBuf {
    path.components()
        .fold(PathBuf::new(), |mut result, p| match p {
            Component::Normal(x) => {
                result.push(x);
                result
            }
            Component::ParentDir => {
                result.pop();
                result
            }
            _ => result,
        })
}

/// Resolved request path.
pub struct RequestedPath {
    /// Fully resolved filesystem path of the request.
    pub full_path: PathBuf,
    /// Whether a directory was requested. (`original` ends with a slash.)
    pub is_dir_request: bool,
}

impl RequestedPath {
    /// Resolve the requested path to a full filesystem path, limited to the root.
    pub fn resolve(root_path: &Path, request_path: &str) -> Self {
        let is_dir_request = request_path.as_bytes().last() == Some(&b'/');
        let request_path = PathBuf::from(decode_percents(request_path));

        let mut full_path = root_path.to_path_buf();
        full_path.extend(&normalize_path(&request_path));

        RequestedPath {
            full_path,
            is_dir_request,
        }
    }
}
