use bytes::{BufMut, BytesMut};
use futures_util::stream::Stream;
use hyper::body::{Body, Bytes};
use std::fs::File;
use std::future::Future;
use std::io::{self, Read};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::task::{spawn_blocking, JoinHandle};

const BUF_SIZE: usize = 8 * 1024;

/// Wraps a `std::fs::File`, and implements an async stream of `Bytes`s.
pub struct FileBytesStream {
    file: Arc<File>,
    state: State,
}

enum State {
    Invalid,
    Idle(BytesMut),
    Busy(JoinHandle<(io::Result<usize>, BytesMut)>),
}

impl FileBytesStream {
    /// Create a new stream from the given file.
    pub fn new(file: File) -> FileBytesStream {
        let file = Arc::new(file);
        let state = State::Idle(BytesMut::new());
        FileBytesStream { file, state }
    }
}

impl Stream for FileBytesStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        loop {
            let mut state = State::Invalid;
            std::mem::swap(&mut state, &mut self.state);
            match state {
                State::Invalid => {
                    unreachable!();
                }
                State::Idle(mut buf) => {
                    let file = self.file.clone();
                    buf.reserve(BUF_SIZE);
                    self.state = State::Busy(spawn_blocking(move || {
                        let slice = buf.bytes_mut();
                        let slice = unsafe {
                            std::slice::from_raw_parts_mut(
                                slice.as_mut_ptr() as *mut u8,
                                slice.len(),
                            )
                        };
                        let res = (&*file).read(slice);
                        (res, buf)
                    }));
                }
                State::Busy(mut op) => {
                    let (res, mut buf) = match Pin::new(&mut op).poll(cx) {
                        Poll::Ready(res) => res?,
                        Poll::Pending => {
                            self.state = State::Busy(op);
                            return Poll::Pending;
                        }
                    };
                    match res {
                        Ok(0) => {
                            self.state = State::Idle(buf);
                            return Poll::Ready(None);
                        }
                        Ok(size) => {
                            unsafe { buf.advance_mut(size) };
                            let retval = buf.split().freeze();
                            self.state = State::Idle(buf);
                            return Poll::Ready(Some(Ok(retval)));
                        }
                        Err(e) => {
                            self.state = State::Idle(buf);
                            return Poll::Ready(Some(Err(e)));
                        }
                    }
                }
            }
        }
    }
}

impl FileBytesStream {
    /// Create a Hyper `Body` from this stream.
    pub fn into_body(self) -> Body {
        Body::wrap_stream(self)
    }
}
