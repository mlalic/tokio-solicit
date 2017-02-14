//! The module exposes trait implementations that allow the Tokio-based HTTP/2 to hook into
//! `solicit`'s HTTP/2 protocol handling.
//!
//! Namely, this means that it implements the `SendFrame` and `ReceiveFrame` traits, which are
//! provided to the handler methods of `solicit`'s `HttpConnection`.
//!
//! Through these traits, `solicit` remains decoupled from the actual mechanism of performing
//! IO and acts as an "interpreter" of the HTTP/2 protocol itself.
//!
//! While these operations are inherently coupled to the underlying IO mechanism, `solicit`
//! provides helpers (mainly in `solicit::http::frame`) that can be used to avoid re-implementing
//! common pieces, such as the actual frame parsing from raw bytes, once they're obtained from
//! the underlying IO, or serializing a frame into bytes prior to writing them out.

use std::io::{self, Read, Write};
use std::collections::VecDeque;

use futures::{Async};
use tokio_core::io::{Io, ReadHalf, WriteHalf};

use solicit::http::{HttpResult};
use solicit::http::frame::{RawFrame, FrameIR};
use solicit::http::connection::{SendFrame, ReceiveFrame, HttpFrame};

/// The struct that implements the `SendFrame` trait.
pub struct FrameSender<T: Io + 'static> {
    /// The write end of a `tokio_core::io::Io` that the sender will attempt to write the raw
    /// bytes to.
    io: WriteHalf<T>,
    /// The current buffer being sent out.
    out_buf: Option<io::Cursor<Vec<u8>>>,
    /// Pending serialized frames
    out_frames: VecDeque<Vec<u8>>,
}

impl<T: Io + 'static> FrameSender<T> {
    /// Creates a new `FrameSender` that will write onto the given `WriteHalf` of a socket
    /// (or rather `tokio_core::io::Io`).
    pub fn new(io: WriteHalf<T>) -> FrameSender<T> {
        FrameSender {
            io: io,
            out_buf: None,
            out_frames: VecDeque::new(),
        }
    }

    /// Adds a serialized frame to the pending frame buffer. It does not attempt writing
    /// anything to the underlying socket.
    fn append(&mut self, b: Vec<u8>) {
        self.out_frames.push_back(b);
    }

    /// Attempts to perform a write of all remaining buffered data. This includes the current
    /// `out_buf`, as well as any pending serialized frames in `out_frames`.
    ///
    /// If it successfully writes all pending data without blocking returns `true`. If it is
    /// unable to write out everything before blocking, returns `false`. Any IO errors (other
    /// than WouldBlock) are propagated out.
    pub fn try_write(&mut self) -> io::Result<bool> {
        trace!("trying to write more buffered data");

        let mut would_block = false;
        while self.prepare_next() {
            match self.io.poll_write() {
                Async::NotReady => {
                    trace!("wouldblock");
                    would_block = true;
                    break;
                }
                Async::Ready(_) => {
                    if !self.write_next()? {
                        // If we didn't manage to write more, break out. We'll try
                        // again later when we get the event-triggered callback.
                        would_block = true;
                        break;
                    }
                }
            }
        }

        // If we didn't block, it means everything got pushed out.
        Ok(!would_block)
    }

    // Returns true if it managed to write some content out.
    // False => nothing written, wouldblock. We'll get a callback scheduled for our
    // troubles to try again later.
    /// Attempts to write as much of the buffered content in `out_buf` as possible.
    ///
    /// Returns `true` if it managed to write anything without blocking. Returns `false`
    /// only if nothing was written out.
    fn write_next(&mut self) -> io::Result<bool> {
        let mut done = false;
        match self.out_buf.as_mut() {
            None => return Ok(true),
            Some(out_buf) => {
                match self.io.write(&out_buf.get_ref()[out_buf.position() as usize..]) {
                    Ok(count) => {
                        debug!("wrote {} bytes", count);

                        let total_written = (out_buf.position() as usize) + count;
                        if total_written == out_buf.get_ref().len() {
                            // All done with the buffer
                            done = true;
                        }
                    },
                    Err(e) => {
                        if e.kind() == io::ErrorKind::WouldBlock {
                            trace!("write would block");
                            return Ok(false);
                        } else {
                            return Err(e);
                        }
                    }
                }
            }
        };

        if done {
            self.out_buf = None;
        }

        Ok(true)
    }

    /// Prepares the `out_buf` for an upcoming write. If the buffer is empty, it attempts to
    /// take the next serialized frame from the pending frames buffer and write that. If there
    /// are still unwritten bytes in the `out_buf`, it immediately returns.
    ///
    /// Returns `true` if there are more bytes to send, which are now ready in `out_buf`.
    /// Returns `false` if there are no more bytes to send.
    fn prepare_next(&mut self) -> bool {
        if self.out_buf.is_none() {
            match self.out_frames.pop_front() {
                // We've written out everything that we had.
                None => false,

                // Get the next frame and make it the out buffer.
                Some(buf) => {
                    self.out_buf = Some(io::Cursor::new(buf));
                    true
                }
            }
        } else {
            // The `out_buf` is still active, so there's obviously more stuff to send.
            true
        }
    }
}

impl<T: Io + 'static> SendFrame for FrameSender<T> {
    fn send_frame<F: FrameIR>(&mut self, frame: F) -> HttpResult<()> {
        // Let solicit's frame serialization do the heavy lifting by serializing the result
        // into a new buffer...
        let mut buf = io::Cursor::new(Vec::with_capacity(1024));
        frame.serialize_into(&mut buf)?;

        // ...and then simply queue that up for the actual wire IO later on.
        self.append(buf.into_inner());

        Ok(())
    }
}

/// A simple wrapper around a parsed `RawFrame`.
///
/// Implements the `ReceiveFrame` trait by trivially yielding the wrapped frame.
///
/// It achieves this without any copies of the original `RawFrame`; if the `RawFrame` is a view
/// over a different buffer (i.e. it borrows it/a part of it), the `FrameContainer` will not
/// cause any copies behind the scenes.
pub struct FrameContainer<'a> {
    frame: RawFrame<'a>,
    size: usize,
}

impl<'a> FrameContainer<'a> {
    /// Creates a new `FrameContainer` that wraps the given `RawFrame`. This frame will be
    /// returned when the `recv_frame` call comes in.
    fn new(frame: RawFrame) -> FrameContainer {
        let size = frame.len();

        FrameContainer {
            frame: frame,
            size: size,
        }
    }

    /// Returns the byte length of the frame that it wraps.
    pub fn len(&self) -> usize {
        self.size
    }
}

impl<'a> ReceiveFrame for FrameContainer<'a> {
    fn recv_frame(&mut self) -> HttpResult<HttpFrame> {
        HttpFrame::from_raw(&self.frame)
    }
}

/// The struct that implements actually parsing a frame off the wire, leveraging tokio_core
/// IO primitives.
pub struct FrameReceiver<T: Io + 'static> {
    /// The ReadHalf of the Tokio `Io` that this receiver will attempt to read from.
    io: ReadHalf<T>,
    /// A buffer of data that has been read so far, but represents an incomplete HTTP/2 frame.
    in_buf: Vec<u8>,
}

impl<T: Io + 'static> FrameReceiver<T> {
    /// Create a new `FrameReceiver` that will use the given `ReadHalf` to read from.
    pub fn new(io: ReadHalf<T>) -> FrameReceiver<T> {
        FrameReceiver {
            io: io,
            in_buf: Vec::new(),
        }
    }

    /// Attempts to read from the underlying socket.
    ///
    /// Returns the number of bytes read. If it would be unable to perform a read without
    /// blocking, immediately returns with 0. If it hits an EOF, it returns with an error, as the
    /// fact that this was called indicates that we were expecting more data on the wire and
    /// therefore we've hit an unexpected eof.
    pub fn try_read(&mut self) -> io::Result<usize> {
        let initial_size = self.in_buf.len();
        loop {
            match self.io.read_to_end(&mut self.in_buf) {
                Ok(0) => {
                    trace!("unexpected eof");
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                },
                Ok(_) => {},
                Err(e) => {
                    if e.kind() == io::ErrorKind::WouldBlock {
                        trace!("read - would block");
                        break;
                    } else {
                        return Err(e);
                    }
                }
            };
        }

        // The buffer could have only grown in the meantime, therefore this is safe from
        // underflow.
        let total_read = self.in_buf.len() - initial_size;
        debug!("total bytes read {}", total_read);

        Ok(total_read)
    }

    /// Attempts to parse the current contents of the input buffer for an h2 frame.
    /// If successful returns a FrameContainer wrapping the frame. The returned `FrameContainer`
    /// will be borrowing the content of the internal buffer, i.e. parsing the frame does not
    /// require creating a new temporary copy of the frame in a new buffer.
    pub fn get_next_frame(&mut self) -> Option<FrameContainer> {
        let raw_frame = RawFrame::parse(&self.in_buf);
        raw_frame.map(|raw_frame| FrameContainer::new(raw_frame))
    }

    /// Discards the current frame from the buffered input.
    pub fn discard_frame(&mut self, size: usize) {
        self.in_buf.drain(..size);
    }
}

