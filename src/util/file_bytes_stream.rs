use std::{
    fmt::Write,
    io::{Error as IoError, SeekFrom},
    pin::Pin,
    task::{Context, Poll},
    vec,
};

use futures_util::stream::Stream;
use http_range::HttpRange;
use hyper::body::{Body, Bytes};

use crate::vfs::FileAccess;

/// Wraps a `FileAccess`, like a tokio `File`, and implements a stream of `Bytes`s.
pub struct FileBytesStream<F> {
    file: F,
    remaining: u64,
}

impl<F> FileBytesStream<F> {
    /// Create a new stream from the given file.
    pub fn new(file: F) -> Self {
        Self {
            file,
            remaining: u64::MAX,
        }
    }

    /// Create a new stream from the given file, reading up to `limit` bytes.
    pub fn new_with_limit(file: F, limit: u64) -> Self {
        Self {
            file,
            remaining: limit,
        }
    }
}

impl<F: FileAccess> Stream for FileBytesStream<F> {
    type Item = Result<Bytes, IoError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let Self {
            ref mut file,
            ref mut remaining,
        } = *self;

        match Pin::new(file).poll_read(cx, *remaining as usize) {
            Poll::Ready(Ok(buf)) => {
                *remaining -= buf.len() as u64;
                if buf.is_empty() {
                    Poll::Ready(None)
                } else {
                    Poll::Ready(Some(Ok(buf)))
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<F: FileAccess> FileBytesStream<F> {
    /// Create a Hyper `Body` from this stream.
    pub fn into_body(self) -> Body {
        Body::wrap_stream(self)
    }
}

#[derive(PartialEq, Eq)]
enum FileSeekState {
    NeedSeek,
    Seeking,
    Reading,
}

/// Wraps a `FileAccess`, like a tokio `File`, and implements a stream of `Bytes`s reading a
/// portion of the file given by `range`.
pub struct FileBytesStreamRange<F> {
    file_stream: FileBytesStream<F>,
    seek_state: FileSeekState,
    start_offset: u64,
}

impl<F> FileBytesStreamRange<F> {
    /// Create a new stream from the given file and range
    pub fn new(file: F, range: HttpRange) -> Self {
        Self {
            file_stream: FileBytesStream::new_with_limit(file, range.length),
            seek_state: FileSeekState::NeedSeek,
            start_offset: range.start,
        }
    }

    fn without_initial_range(file: F) -> Self {
        Self {
            file_stream: FileBytesStream::new_with_limit(file, 0),
            seek_state: FileSeekState::NeedSeek,
            start_offset: 0,
        }
    }
}

impl<F: FileAccess> Stream for FileBytesStreamRange<F> {
    type Item = Result<Bytes, IoError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let Self {
            ref mut file_stream,
            ref mut seek_state,
            start_offset,
        } = *self;
        if *seek_state == FileSeekState::NeedSeek {
            *seek_state = FileSeekState::Seeking;
            if let Err(e) =
                Pin::new(&mut file_stream.file).start_seek(SeekFrom::Start(start_offset))
            {
                return Poll::Ready(Some(Err(e)));
            }
        }
        if *seek_state == FileSeekState::Seeking {
            match Pin::new(&mut file_stream.file).poll_complete(cx) {
                Poll::Ready(Ok(..)) => *seek_state = FileSeekState::Reading,
                Poll::Ready(Err(e)) => return Poll::Ready(Some(Err(e))),
                Poll::Pending => return Poll::Pending,
            }
        }
        Pin::new(file_stream).poll_next(cx)
    }
}

impl<F: FileAccess> FileBytesStreamRange<F> {
    /// Create a Hyper `Body` from this stream.
    pub fn into_body(self) -> Body {
        Body::wrap_stream(self)
    }
}

/// Wraps a `FileAccess`, like a tokio `File`,  and implements a stream of `Bytes`s reading
/// multiple portions of the file given by `ranges` using a chunked multipart/byteranges response.
/// A boundary is required to separate the chunked components and therefore needs to be unlikely to
/// be in any file.
pub struct FileBytesStreamMultiRange<F> {
    file_range: FileBytesStreamRange<F>,
    range_iter: vec::IntoIter<HttpRange>,
    is_first_boundary: bool,
    completed: bool,
    boundary: String,
    content_type: String,
    file_length: u64,
}

impl<F> FileBytesStreamMultiRange<F> {
    /// Create a new stream from the given file, ranges, boundary and file length.
    pub fn new(file: F, ranges: Vec<HttpRange>, boundary: String, file_length: u64) -> Self {
        Self {
            file_range: FileBytesStreamRange::without_initial_range(file),
            range_iter: ranges.into_iter(),
            boundary,
            is_first_boundary: true,
            completed: false,
            content_type: String::new(),
            file_length,
        }
    }

    /// Set the Content-Type header in the multipart/byteranges chunks.
    pub fn set_content_type(&mut self, content_type: &str) {
        self.content_type = content_type.to_string();
    }

    /// Computes the length of the body for the multi-range response being produced by this
    /// `FileBytesStreamMultiRange`.
    pub fn compute_length(&self) -> u64 {
        let Self {
            ref range_iter,
            ref boundary,
            ref content_type,
            file_length,
            ..
        } = *self;

        let mut total_length = 0;
        let mut is_first = true;
        for range in range_iter.as_slice() {
            let header =
                render_multipart_header(boundary, content_type, *range, is_first, file_length);

            is_first = false;
            total_length += header.as_bytes().len() as u64;
            total_length += range.length;
        }

        let header = render_multipart_header_end(boundary);
        total_length += header.as_bytes().len() as u64;

        total_length
    }
}

fn render_multipart_header(
    boundary: &str,
    content_type: &str,
    range: HttpRange,
    is_first: bool,
    file_length: u64,
) -> String {
    let mut buf = String::with_capacity(128);
    if !is_first {
        buf.push_str("\r\n");
    }
    write!(
        &mut buf,
        "--{boundary}\r\nContent-Range: bytes {}-{}/{file_length}\r\n",
        range.start,
        range.start + range.length - 1,
    )
    .expect("buffer write failed");

    if !content_type.is_empty() {
        write!(&mut buf, "Content-Type: {content_type}\r\n").expect("buffer write failed");
    }

    buf.push_str("\r\n");
    buf
}

fn render_multipart_header_end(boundary: &str) -> String {
    format!("\r\n--{boundary}--\r\n")
}

impl<F: FileAccess> Stream for FileBytesStreamMultiRange<F> {
    type Item = Result<Bytes, IoError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let Self {
            ref mut file_range,
            ref mut range_iter,
            ref mut is_first_boundary,
            ref mut completed,
            ref boundary,
            ref content_type,
            file_length,
        } = *self;

        if *completed {
            return Poll::Ready(None);
        }

        if file_range.file_stream.remaining == 0 {
            let range = match range_iter.next() {
                Some(r) => r,
                None => {
                    *completed = true;

                    let header = render_multipart_header_end(boundary);
                    return Poll::Ready(Some(Ok(header.into())));
                }
            };

            file_range.seek_state = FileSeekState::NeedSeek;
            file_range.start_offset = range.start;
            file_range.file_stream.remaining = range.length;

            let cur_is_first = *is_first_boundary;
            *is_first_boundary = false;

            let header =
                render_multipart_header(boundary, content_type, range, cur_is_first, file_length);
            return Poll::Ready(Some(Ok(header.into())));
        }

        Pin::new(file_range).poll_next(cx)
    }
}

impl<F: FileAccess> FileBytesStreamMultiRange<F> {
    /// Create a Hyper `Body` from this stream.
    pub fn into_body(self) -> Body {
        Body::wrap_stream(self)
    }
}
