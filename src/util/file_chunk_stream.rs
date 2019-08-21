use futures::{Async, Poll, Stream};
use hyper::Chunk;
use std::io::Error;
use std::mem;
use tokio::fs::File;
use tokio::io::AsyncRead;

const BUF_SIZE: usize = 8 * 1024;

/// Wraps a `tokio::fs::File`, and implements a stream of `hyper::Chunk`s.
pub struct FileChunkStream {
    file: File,
    buf: Box<[u8; BUF_SIZE]>,
}

impl FileChunkStream {
    /// Create a new stream from the given file.
    pub fn new(file: File) -> FileChunkStream {
        let buf = Box::new(unsafe { mem::uninitialized() });
        FileChunkStream { file, buf }
    }
}

impl Stream for FileChunkStream {
    type Item = Chunk;
    type Error = Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.file.poll_read(&mut self.buf[..]) {
            Ok(Async::Ready(0)) => Ok(Async::Ready(None)),
            Ok(Async::Ready(size)) => Ok(Async::Ready(Some(self.buf[..size].to_owned().into()))),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(e) => Err(e),
        }
    }
}
