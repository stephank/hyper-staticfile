use std::{
    io::Error as IoError,
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_util::stream::Stream;
use hyper::body::{Bytes, Frame};
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncSeek},
};

use crate::util::{FileBytesStream, FileBytesStreamMultiRange, FileBytesStreamRange};

/// Hyper Body implementation for the various types of streams used in static serving.
pub enum Body<F = File> {
    /// No response body.
    Empty,
    /// Serve a complete file.
    Full(FileBytesStream<F>),
    /// Serve a range from a file.
    Range(FileBytesStreamRange<F>),
    /// Serve multiple ranges from a file.
    MultiRange(FileBytesStreamMultiRange<F>),
}

impl<F> hyper::body::Body for Body<F>
where
    F: AsyncRead + AsyncSeek + Unpin,
{
    type Data = Bytes;
    type Error = IoError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, IoError>>> {
        let opt = ready!(match *self {
            Body::Empty => return Poll::Ready(None),
            Body::Full(ref mut stream) => Pin::new(stream).poll_next(cx),
            Body::Range(ref mut stream) => Pin::new(stream).poll_next(cx),
            Body::MultiRange(ref mut stream) => Pin::new(stream).poll_next(cx),
        });
        Poll::Ready(opt.map(|res| res.map(Frame::data)))
    }
}
