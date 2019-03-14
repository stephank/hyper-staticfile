use futures::{Async::*, Future, Poll};
use std::fs::{Metadata, OpenOptions as StdOpenOptions};
use std::io::Error;
use std::path::PathBuf;
use tokio::fs::{File, OpenOptions, file::OpenFuture};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
#[cfg(windows)]
use winapi::um::winbase::FILE_FLAG_BACKUP_SEMANTICS;

/// State of `open_with_metadata` as it progresses.
enum OpenWithMetadataState {
    /// Wait for file to open.
    WaitOpen(OpenFuture<PathBuf>),
    /// Wait for metadata on the file.
    WaitMetadata,
    /// Finished.
    Done,
}

/// Future returned by `open_with_metadata`.
pub struct OpenWithMetadataFuture {
    /// Current state of this future.
    state: OpenWithMetadataState,
    /// Resulting file handle.
    file: Option<File>,
    /// Resulting file metadata.
    metadata: Option<Metadata>,
}

impl Future for OpenWithMetadataFuture {
    type Item = (File, Metadata);
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            self.state = match self.state {
                OpenWithMetadataState::WaitOpen(ref mut future) => {
                    self.file = Some(try_ready!(future.poll()));
                    OpenWithMetadataState::WaitMetadata
                },
                OpenWithMetadataState::WaitMetadata => {
                    let file = self.file.as_mut().expect("invalid state");
                    self.metadata = Some(try_ready!(file.poll_metadata()));
                    OpenWithMetadataState::Done
                },
                OpenWithMetadataState::Done => {
                    let file = self.file.take().expect("invalid state");
                    let metadata = self.metadata.take().expect("invalid state");
                    return Ok(Ready((file, metadata)));
                },
            }
        }
    }
}

/// Open a file and get metadata.
pub fn open_with_metadata(path: PathBuf) -> OpenWithMetadataFuture {
    let mut opts = StdOpenOptions::new();
    opts.read(true);

    // On Windows, we need to set this flag to be able to open directories.
    #[cfg(windows)]
    opts.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);

    let state = OpenWithMetadataState::WaitOpen(OpenOptions::from(opts).open(path));
    OpenWithMetadataFuture { state, file: None, metadata: None }
}
