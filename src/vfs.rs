use futures_util::FutureExt;
use std::{
    fs::OpenOptions,
    future::Future,
    io::{Error, ErrorKind},
    path::{Path, PathBuf},
    time::SystemTime,
};
use tokio::{fs::File, task::spawn_blocking};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
#[cfg(windows)]
use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;

/// Open file handle with metadata.
///
/// This struct exists because we want to abstract away tokio `File`, but need to use
/// `File`-specific operations to find the metadata and fill the additional fields here.
///
/// This struct is eventually converted to a `ResolvedFile`.
pub struct FileWithMetadata<F = File> {
    /// Open file handle.
    pub handle: F,
    /// Size in bytes.
    pub size: u64,
    /// Last modification time.
    pub modified: Option<SystemTime>,
    /// Whether this is a directory.
    pub is_dir: bool,
}

/// Trait for a simple virtual filesystem layer.
///
/// There is only the `open` operation, hence the name `FileOpener`. In practice, `open` must also
/// collect some file metadata. (See the `FileWithMetadata` struct.)
///
/// In order to use an implementation with the other parts of this crate (ie. resolver and
/// response builders), it must be marked `Send` and `Sync`, and must have `'static` lifetime.
pub trait FileOpener {
    /// File handle type.
    ///
    /// In order to use files with the other parts of this crate, the file handle must implement
    /// the `AsyncRead` and `AsyncSeek` traits, must be marked `Send` and `Unpin`, and have
    /// `'static` lifetime.
    type File;

    /// Future type that `open` returns.
    ///
    /// This future must be marked `Send` in order to be used with other parts of this crate.
    type Future: Future<Output = Result<FileWithMetadata<Self::File>, Error>>;

    /// Open a file and return a `FileWithMetadata`.
    ///
    /// It can be assumed the path is already sanitized at this point.
    fn open(&self, path: &Path) -> Self::Future;
}

/// Filesystem implementation that uses `tokio::fs`.
pub struct TokioFileOpener {
    /// The virtual root directory to use when opening files.
    ///
    /// The path may be absolute or relative.
    pub root: PathBuf,
}

impl TokioFileOpener {
    /// Create a new `TokioFileOpener` for the given root path.
    ///
    /// The path may be absolute or relative.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl FileOpener for TokioFileOpener {
    type File = File;
    type Future = Box<dyn Future<Output = Result<FileWithMetadata<File>, Error>> + Send + Unpin>;

    /// Open a file with tokio.
    fn open(&self, path: &Path) -> Self::Future {
        let mut full_path = self.root.clone();
        full_path.extend(path);

        Box::new(
            spawn_blocking(move || {
                let mut opts = OpenOptions::new();
                opts.read(true);

                // On Windows, we need to set this flag to be able to open directories.
                #[cfg(windows)]
                opts.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);

                let handle = opts.open(full_path)?;
                let metadata = handle.metadata()?;
                Ok(FileWithMetadata {
                    handle: File::from_std(handle),
                    size: metadata.len(),
                    modified: metadata.modified().ok(),
                    is_dir: metadata.is_dir(),
                })
            })
            .map(|res| match res {
                Ok(res) => res,
                Err(_) => Err(Error::new(ErrorKind::Other, "background task failed")),
            }),
        )
    }
}
