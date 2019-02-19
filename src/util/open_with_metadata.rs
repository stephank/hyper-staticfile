use futures::{Async::*, Future, Poll};
use std::fs::Metadata;
use std::io::Error;
use std::path::PathBuf;
use tokio::fs::{File, metadata};
use tokio_fs::MetadataFuture;

/// State of `open_with_metadata` as it progresses.
enum OpenWithMetadataState {
    /// Wait for file to open.
    WaitOpen,
    /// Wait for metadata on the file.
    WaitMetadata(MetadataFuture<PathBuf>),
    /// Finished.
    Done,
}

/// Future returned by `open_with_metadata`.
pub struct OpenWithMetadataFuture {
    /// Current state of this future.
    state: OpenWithMetadataState,
    /// path of file to load
    path: PathBuf,
    /// Resulting file handle.
    file: Option<File>,
    /// Resulting file metadata.
    metadata: Option<Metadata>,
}

impl Future for OpenWithMetadataFuture {
    type Item = (Option<File>, Metadata);
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            self.state = match self.state {
                OpenWithMetadataState::WaitMetadata(ref mut future) => {
                    self.metadata = Some(try_ready!(future.poll()));
                    OpenWithMetadataState::WaitOpen
                },
                OpenWithMetadataState::WaitOpen => {
                    if self.metadata.clone().expect("Could not read file metadata").is_file() {
                        self.file = Some(try_ready!(File::open(self.path.clone()).poll()));
                    }
                    OpenWithMetadataState::Done
                },
                OpenWithMetadataState::Done => {
                    let file = self.file.take();
                    let metadata = self.metadata.take().expect("invalid state");
                    return Ok(Ready((file, metadata)));
                },
            }
        }
    }
}

/// Open a file and get metadata.
pub fn open_with_metadata(path: PathBuf) -> OpenWithMetadataFuture {
    let state = OpenWithMetadataState::WaitMetadata(metadata(path.clone()));
    OpenWithMetadataFuture { state, path, file: None, metadata: None }
}
