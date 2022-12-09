use futures_util::future::{ready, FutureExt, Ready};
use hyper::body::Bytes;
use std::{
    collections::HashMap,
    fs::OpenOptions,
    future::Future,
    io::{Cursor, Error, ErrorKind},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::{
    fs::{self, File},
    task::spawn_blocking,
};

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
#[derive(Debug)]
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

type MemoryFileMap = HashMap<PathBuf, FileWithMetadata<Bytes>>;

/// Builder for a `MemoryFs`.
pub struct MemoryFsBuilder {
    files: MemoryFileMap,
}

impl Default for MemoryFsBuilder {
    fn default() -> Self {
        let mut files = MemoryFileMap::new();

        // Create a top-level directory entry.
        files.insert(
            PathBuf::new(),
            FileWithMetadata {
                handle: Bytes::new(),
                size: 0,
                modified: None,
                is_dir: true,
            },
        );

        Self { files }
    }
}

impl MemoryFsBuilder {
    /// Add a file to the `MemoryFs`.
    ///
    /// This automatically creates directory entries leading up to the path. Any existing entries
    /// are overwritten.
    pub fn add(
        &mut self,
        path: impl Into<PathBuf>,
        data: Bytes,
        modified: Option<SystemTime>,
    ) -> &mut Self {
        let path = path.into();

        // Create directory entries.
        let mut components: Vec<_> = path.components().collect();
        components.pop();
        let mut dir_path = PathBuf::new();
        for component in components {
            if let Component::Normal(x) = component {
                dir_path.push(x);
                self.files.insert(
                    dir_path.clone(),
                    FileWithMetadata {
                        handle: Bytes::new(),
                        size: 0,
                        modified: None,
                        is_dir: true,
                    },
                );
            }
        }

        // Create the file entry.
        let size = data.len() as u64;
        self.files.insert(
            path,
            FileWithMetadata {
                handle: data,
                size,
                modified,
                is_dir: false,
            },
        );

        self
    }

    /// Consume the builder and return the `MemoryFs`.
    pub fn build(self) -> MemoryFs {
        MemoryFs {
            files: Arc::new(self.files),
        }
    }
}

/// An in-memory virtual filesystem.
///
/// This type implements `FileOpener`, and can be directly used in `Static::with_opener`, for example.
#[derive(Default)]
pub struct MemoryFs {
    files: Arc<MemoryFileMap>,
}

impl MemoryFs {
    /// Create a new builder.
    ///
    /// Alias for: `MemoryFsBuilder::default()`
    pub fn builder() -> MemoryFsBuilder {
        MemoryFsBuilder::default()
    }

    /// Initialize a `MemoryFs` from a directory.
    ///
    /// This loads all files and their contents into memory. Symlinks are followed.
    pub async fn from_dir(path: impl AsRef<Path>) -> Result<Self, Error> {
        let mut builder = Self::builder();

        // Pending directories to scan, as: `(real path, virtual path)`
        let mut dirs = vec![(path.as_ref().to_path_buf(), PathBuf::new())];
        while let Some((dir, base)) = dirs.pop() {
            let mut iter = fs::read_dir(dir).await?;
            while let Some(entry) = iter.next_entry().await? {
                let metadata = entry.metadata().await?;

                // Build the virtual path.
                let mut out_path = base.to_path_buf();
                out_path.push(entry.file_name());

                if metadata.is_dir() {
                    // Add to pending stack,
                    dirs.push((entry.path(), out_path));
                } else if metadata.is_file() {
                    // Read file contents and create an entry.
                    let data = fs::read(entry.path()).await?;
                    builder.add(out_path, data.into(), metadata.modified().ok());
                }
            }
        }

        Ok(builder.build())
    }
}

impl FileOpener for MemoryFs {
    type File = Cursor<Bytes>;
    type Future = Ready<Result<FileWithMetadata<Self::File>, Error>>;

    fn open(&self, path: &Path) -> Self::Future {
        ready(
            self.files
                .get(path)
                .map(|file| FileWithMetadata {
                    handle: Cursor::new(file.handle.clone()),
                    size: file.size,
                    modified: file.modified,
                    is_dir: file.is_dir,
                })
                .ok_or_else(|| Error::new(ErrorKind::NotFound, "Not found")),
        )
    }
}
