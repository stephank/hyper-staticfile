use futures_util::stream::Stream;
use http_range::HttpRange;
use hyper::body::{Body, Bytes};
use std::cmp::min;
use std::io::{Cursor, Error as IoError, SeekFrom, Write};
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::vec;
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};

const BUF_SIZE: usize = 8 * 1024;

/// Wraps a `tokio::fs::File`, and implements a stream of `Bytes`s.
pub struct FileBytesStream {
    file: File,
    buf: Box<[MaybeUninit<u8>; BUF_SIZE]>,
    remaining: u64,
}

impl FileBytesStream {
    /// Create a new stream from the given file.
    pub fn new(file: File) -> FileBytesStream {
        let buf = Box::new([MaybeUninit::uninit(); BUF_SIZE]);
        FileBytesStream {
            file,
            buf,
            remaining: u64::MAX,
        }
    }

    /// Create a new stream from the given file, reading up to `limit` bytes.
    pub fn new_with_limit(file: File, limit: u64) -> FileBytesStream {
        let buf = Box::new([MaybeUninit::uninit(); BUF_SIZE]);
        FileBytesStream {
            file,
            buf,
            remaining: limit,
        }
    }
}

impl Stream for FileBytesStream {
    type Item = Result<Bytes, IoError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let Self {
            ref mut file,
            ref mut buf,
            ref mut remaining,
        } = *self;

        let max_read_length = min(*remaining, buf.len() as u64) as usize;
        let mut read_buf = ReadBuf::uninit(&mut buf[..max_read_length]);
        match Pin::new(file).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => {
                let filled = read_buf.filled();
                *remaining -= filled.len() as u64;
                if filled.is_empty() {
                    Poll::Ready(None)
                } else {
                    Poll::Ready(Some(Ok(Bytes::copy_from_slice(filled))))
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl FileBytesStream {
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

/// Wraps a `tokio::fs::File` and implements a stream of `Bytes`s reading a portion of the file.
pub struct FileBytesStreamRange {
    file_stream: FileBytesStream,
    seek_state: FileSeekState,
    start_offset: u64,
}

impl FileBytesStreamRange {
    /// Create a new stream from the given file and range.
    pub fn new(file: File, range: HttpRange) -> FileBytesStreamRange {
        FileBytesStreamRange {
            file_stream: FileBytesStream::new_with_limit(file, range.length),
            seek_state: FileSeekState::NeedSeek,
            start_offset: range.start,
        }
    }

    fn without_initial_range(file: File) -> FileBytesStreamRange {
        FileBytesStreamRange {
            file_stream: FileBytesStream::new_with_limit(file, 0),
            seek_state: FileSeekState::NeedSeek,
            start_offset: 0,
        }
    }
}

impl Stream for FileBytesStreamRange {
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

impl FileBytesStreamRange {
    /// Create a Hyper `Body` from this stream.
    pub fn into_body(self) -> Body {
        Body::wrap_stream(self)
    }
}

/// Wraps a `tokio::fs::File` and implements a stream of `Bytes`s providing multiple ranges of the
/// file contents in HTTP chunked transfer encoding.
pub struct FileBytesStreamMultiRange {
    file_range: FileBytesStreamRange,
    range_iter: vec::IntoIter<HttpRange>,
    is_first_boundary: bool,
    completed: bool,
    boundary: String,
    content_type: String,
    file_length: u64,
}

impl FileBytesStreamMultiRange {
    /// Create a new stream from the given file, ranges, boundary and file length.
    ///
    /// A boundary is required to separate the chunked components and therefore needs to be
    /// unlikely to be in any file.
    pub fn new(
        file: File,
        ranges: Vec<HttpRange>,
        boundary: String,
        file_length: u64,
    ) -> FileBytesStreamMultiRange {
        FileBytesStreamMultiRange {
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
    /// `FileBytesStreamMultiRange`.  This function is required to be mutable because it temporarily
    /// uses pre-allocated buffers.
    pub fn compute_length(&mut self) -> u64 {
        let Self {
            ref mut file_range,
            ref range_iter,
            ref boundary,
            ref content_type,
            file_length,
            ..
        } = *self;

        let mut total_length = 0;
        let mut is_first = true;
        for range in range_iter.as_slice() {
            let mut read_buf = ReadBuf::uninit(&mut file_range.file_stream.buf[..]);
            render_multipart_header(
                &mut read_buf,
                boundary,
                content_type,
                *range,
                is_first,
                file_length,
            );

            is_first = false;
            total_length += read_buf.filled().len() as u64;
            total_length += range.length;
        }

        let mut read_buf = ReadBuf::uninit(&mut file_range.file_stream.buf[..]);
        render_multipart_header_end(&mut read_buf, boundary);
        total_length += read_buf.filled().len() as u64;

        total_length
    }
}

fn render_multipart_header(
    read_buf: &mut ReadBuf<'_>,
    boundary: &str,
    content_type: &str,
    range: HttpRange,
    is_first: bool,
    file_length: u64,
) {
    if !is_first {
        read_buf.put_slice(b"\r\n");
    }
    read_buf.put_slice(b"--");
    read_buf.put_slice(boundary.as_bytes());
    read_buf.put_slice(b"\r\nContent-Range: bytes ");

    // 64 is 20 (max length of 64 bit integer) * 3 + 4 (symbols, new line)
    let mut tmp_buffer = [0; 64];
    let mut tmp_storage = Cursor::new(&mut tmp_buffer[..]);
    write!(
        &mut tmp_storage,
        "{}-{}/{}\r\n",
        range.start,
        range.start + range.length - 1,
        file_length,
    )
    .expect("buffer unexpectedly too small");

    let end_position = tmp_storage.position() as usize;
    let tmp_storage = tmp_storage.into_inner();
    read_buf.put_slice(&tmp_storage[..end_position]);

    if !content_type.is_empty() {
        read_buf.put_slice(b"Content-Type: ");
        read_buf.put_slice(content_type.as_bytes());
        read_buf.put_slice(b"\r\n");
    }

    read_buf.put_slice(b"\r\n");
}

fn render_multipart_header_end(read_buf: &mut ReadBuf<'_>, boundary: &str) {
    read_buf.put_slice(b"\r\n--");
    read_buf.put_slice(boundary.as_bytes());
    read_buf.put_slice(b"--\r\n");
}

impl Stream for FileBytesStreamMultiRange {
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

                    let mut read_buf = ReadBuf::uninit(&mut file_range.file_stream.buf[..]);
                    render_multipart_header_end(&mut read_buf, boundary);
                    return Poll::Ready(Some(Ok(Bytes::copy_from_slice(read_buf.filled()))));
                }
            };

            file_range.seek_state = FileSeekState::NeedSeek;
            file_range.start_offset = range.start;
            file_range.file_stream.remaining = range.length;

            let cur_is_first = *is_first_boundary;
            *is_first_boundary = false;

            let mut read_buf = ReadBuf::uninit(&mut file_range.file_stream.buf[..]);
            render_multipart_header(
                &mut read_buf,
                boundary,
                content_type,
                range,
                cur_is_first,
                file_length,
            );

            return Poll::Ready(Some(Ok(Bytes::copy_from_slice(read_buf.filled()))));
        }

        Pin::new(file_range).poll_next(cx)
    }
}

impl FileBytesStreamMultiRange {
    /// Create a Hyper `Body` from this stream.
    pub fn into_body(self) -> Body {
        Body::wrap_stream(self)
    }
}
