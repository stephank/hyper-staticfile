use futures::{Async::*, Future, Poll};
use std::fs::Metadata;
use std::io::Error;
use std::path::PathBuf;
use tokio::fs::{File, file::OpenFuture};

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
    let state = OpenWithMetadataState::WaitOpen(File::open(path));
    OpenWithMetadataFuture { state, file: None, metadata: None }
}
