use std::{
    cmp::min,
    collections::HashMap,
    fs::OpenOptions,
    future::Future,
    io::{Cursor, Error, ErrorKind},
    mem::MaybeUninit,
    path::{Component, Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
    time::SystemTime,
};

use futures_util::future::{ready, Ready};
use hyper::body::Bytes;
use tokio::{
    fs::{self, File},
    io::{AsyncRead, AsyncSeek, ReadBuf},
    task::{spawn_blocking, JoinHandle},
};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
#[cfg(windows)]
use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;

const TOKIO_READ_BUF_SIZE: usize = 8 * 1024;

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
pub trait FileOpener: Send + Sync + 'static {
    /// File handle type.
    type File: IntoFileAccess;

    /// Future type that `open` returns.
    type Future: Future<Output = Result<FileWithMetadata<Self::File>, Error>> + Send;

    /// Open a file and return a `FileWithMetadata`.
    ///
    /// It can be assumed the path is already sanitized at this point.
    fn open(&self, path: &Path) -> Self::Future;
}

/// Trait that converts a file handle into something that implements `FileAccess`.
///
/// This trait is called when streaming starts, and exists as a separate step so that buffer
/// allocation doesn't have to happen until that point.
pub trait IntoFileAccess: Send + Unpin + 'static {
    /// Target type that implements `FileAccess`.
    type Output: FileAccess;

    /// Convert into a type that implements `FileAccess`.
    fn into_file_access(self) -> Self::Output;
}

/// Trait that implements all the necessary file access methods used for serving files.
///
/// This trait exists as an alternative to `AsyncRead` that returns a `Bytes` directly, potentially
/// eliminating a copy. Unlike `AsyncRead`, this does mean the implementation is responsible for
/// providing the read buffer.
pub trait FileAccess: AsyncSeek + Send + Unpin + 'static {
    /// Attempts to read from the file.
    ///
    /// If no data is available for reading, the method returns `Poll::Pending` and arranges for
    /// the current task (via `cx.waker()`) to receive a notification when the object becomes
    /// readable or is closed.
    ///
    /// An empty `Bytes` return value indicates EOF.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        len: usize,
    ) -> Poll<Result<Bytes, Error>>;
}

//
// Tokio File implementation
//

impl IntoFileAccess for File {
    type Output = TokioFileAccess;

    fn into_file_access(self) -> Self::Output {
        TokioFileAccess::new(self)
    }
}

/// Struct that wraps a tokio `File` to implement `FileAccess`.
pub struct TokioFileAccess {
    file: File,
    read_buf: Box<[MaybeUninit<u8>; TOKIO_READ_BUF_SIZE]>,
}

impl TokioFileAccess {
    /// Create a new `TokioFileAccess` for a `File`.
    pub fn new(file: File) -> Self {
        TokioFileAccess {
            file,
            read_buf: Box::new([MaybeUninit::uninit(); TOKIO_READ_BUF_SIZE]),
        }
    }
}

impl AsyncSeek for TokioFileAccess {
    fn start_seek(mut self: Pin<&mut Self>, position: std::io::SeekFrom) -> std::io::Result<()> {
        Pin::new(&mut self.file).start_seek(position)
    }

    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Pin::new(&mut self.file).poll_complete(cx)
    }
}

impl FileAccess for TokioFileAccess {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        len: usize,
    ) -> Poll<Result<Bytes, Error>> {
        let Self {
            ref mut file,
            ref mut read_buf,
        } = *self;

        let len = min(len, read_buf.len());
        let mut read_buf = ReadBuf::uninit(&mut read_buf[..len]);
        match Pin::new(file).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => {
                let filled = read_buf.filled();
                if filled.is_empty() {
                    Poll::Ready(Ok(Bytes::new()))
                } else {
                    Poll::Ready(Ok(Bytes::copy_from_slice(filled)))
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
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
    type Future = TokioFileFuture;

    fn open(&self, path: &Path) -> Self::Future {
        let mut full_path = self.root.clone();
        full_path.extend(path);

        // Small perf gain: we do open + metadata in one go. If we used the tokio async functions
        // here, that'd amount to two `spawn_blocking` calls behind the scenes.
        let inner = spawn_blocking(move || {
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
        });

        TokioFileFuture { inner }
    }
}

/// Future type produced by `TokioFileOpener`.
///
/// This type mostly exists just to prevent a `Box<dyn Future>`.
pub struct TokioFileFuture {
    inner: JoinHandle<Result<FileWithMetadata<File>, Error>>,
}

impl Future for TokioFileFuture {
    type Output = Result<FileWithMetadata<File>, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // The task produces a result, but so does the `JoinHandle`, so this is a
        // `Result<Result<..>>`. We map the `JoinHandle` error to an IO error, so that we can
        // flatten the results. This is similar to what tokio does, but that just uses `Map` and
        // async functions (with an anonymous future type).
        match Pin::new(&mut self.inner).poll(cx) {
            Poll::Ready(Ok(res)) => Poll::Ready(res),
            Poll::Ready(Err(_)) => {
                Poll::Ready(Err(Error::new(ErrorKind::Other, "background task failed")))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

//
// In-memory implementation
//

type MemoryFileMap = HashMap<PathBuf, FileWithMetadata<Bytes>>;

impl IntoFileAccess for Cursor<Bytes> {
    type Output = Self;

    fn into_file_access(self) -> Self::Output {
        // No read buffer required. We can simply create subslices.
        self
    }
}

impl FileAccess for Cursor<Bytes> {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        len: usize,
    ) -> Poll<Result<Bytes, Error>> {
        let pos = self.position();
        let slice = (*self).get_ref();

        // The position could technically be out of bounds, so don't panic...
        if pos > slice.len() as u64 {
            return Poll::Ready(Ok(Bytes::new()));
        }

        let start = pos as usize;
        let amt = min(slice.len() - start, len);
        // Add won't overflow because of pos check above.
        let end = start + amt;
        Poll::Ready(Ok(slice.slice(start..end)))
    }
}

/// An in-memory virtual filesystem.
///
/// This type implements `FileOpener`, and can be directly used in `Static::with_opener`, for example.
pub struct MemoryFs {
    files: MemoryFileMap,
}

impl Default for MemoryFs {
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

impl MemoryFs {
    /// Initialize a `MemoryFs` from a directory.
    ///
    /// This loads all files and their contents into memory. Symlinks are followed.
    pub async fn from_dir(path: impl AsRef<Path>) -> Result<Self, Error> {
        let mut fs = Self::default();

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
                    fs.add(out_path, data.into(), metadata.modified().ok());
                }
            }
        }

        Ok(fs)
    }

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
