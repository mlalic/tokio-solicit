//! Implements the traits that are required to hook up low level Tokio IO to the Tokio
//! protocol API.
//!
//! The main struct that it exposes is the `H2ClientTokioTransport`, which is the bridge between
//! the semantic representation of an HTTP/2 request/response and the lower-level IO.
//!
//! Also exposes the `H2ClientTokioProto` that allows an existing `Io` instance to be bound
//! to the HTTP/2 Tokio transport, as implemented by `H2ClientTokioTransport`.

use super::{HttpRequestHeaders, HttpRequestBody, HttpResponseHeaders, HttpResponseBody};

use io::{FrameSender, FrameReceiver};

use std::io::{self};
use std::collections::HashMap;

use futures::{Async, AsyncSink, Future, Poll, StartSend};
use futures::future::{self};
use futures::sink::Sink;
use futures::stream::{Stream};
use futures::task;

use tokio_core::io::{Io, self as tokio_io};
use tokio_proto::streaming::multiplex::{ClientProto, Transport, Frame};

use solicit::http::{HttpResult, HttpScheme, StaticHeader};
use solicit::http::connection::{HttpConnection, SendStatus};
use solicit::http::session::{
    Client as ClientMarker,
    Stream as SolicitStream,
    DefaultSessionState,
    SessionState,
    DefaultStream,
};
use solicit::http::client::{self, ClientConnection, RequestStream};

/// A type alias for the Frame type that we need to yield to Tokio from the Transport impl's
/// `Stream`.
type TokioResponseFrame = Frame<HttpResponseHeaders, HttpResponseBody, io::Error>;

/// Implements the Tokio Transport trait -- a layer that translates between the HTTP request
/// and response semantic representations (the Http{Request,Response}{Headers,Body} structs)
/// and the lower-level IO required to drive HTTP/2.
///
/// It handles mapping Tokio-level request IDs to HTTP/2 stream IDs, so that once a response
/// is received, it can notify the correct Tokio request.
///
/// It holds the HTTP/2 connection state, as a `ClientConnection` using `DefaultStream`s.
///
/// The HTTP/2 connection is fed by the `FrameSender` and `FrameReceiver`.
///
/// To satisfy the Tokio Transport trait, this struct implements a `Sink` and a `Stream` trait.
/// # Sink
///
/// As a `Sink`, it acts as a `Sink` of Tokio request frames. These frames do not necessarily
/// have a 1-1 mapping to HTTP/2 frames, but it is the task of this struct to do the required
/// mapping.
///
/// For example, a Tokio frame that signals the start of a new request, `Frame::Message`, will be
/// handled by constructing a new HTTP/2 stream in the HTTP/2 session state and instructing the
/// HTTP/2 client connection to start a new request, which will queue up the required HEADERS
/// frames, signaling the start of a new request.
///
/// When handling body "chunk" Tokio frames, i.e. the `Frame::Body` variant, it will notify the
/// appropriate stream (by mapping the Tokio request ID to the matching HTTP/2 stream ID).
///
/// NOTE: Currently, this relies on the fact that we assume only a single request body "chunk",
///       which contains the entire body of the request.
///
/// # Stream
///
/// As a `Stream`, the struct acts as a `Stream` of `TokioResponseFrame`s. Tokio itself internally
/// knows how to interpret these frames in order to produce:
/// 
///   1. A `Future` that resolves once the response headers come in
///   2. A `Stream` that will yield response body chunks
///
/// Therefore, the job of this struct is to feed the HTTP/2 frames read by the `receiver` from the
/// underlying raw IO to the ClientConnection and subsequently to interpret the new state of the
/// HTTP/2 session in order to produce `TokioResponseFrame`s.
///
/// Currently, for each request we produce only a single `TokioResponseFrame` once the entire
/// request completes. Therefore, the "header" contains not only the HTTP response headers, but
/// also the full body.
/// (NOTE(mlalic): There's nothing inherently blocking this, I just haven't gotten 'round to it,
/// yet.)
pub struct H2ClientTokioTransport<T: Io + 'static> {
    sender: FrameSender<T>,
    receiver: FrameReceiver<T>,
    conn: ClientConnection,
    ready_responses: Vec<(u64, HttpResponseHeaders)>,

    // TODO: Should use a bijective map here to simplify...
    h2stream_to_tokio_request: HashMap<u32, u64>,
    tokio_request_to_h2stream: HashMap<u64, u32>,
}

impl<T> H2ClientTokioTransport<T> where T: Io + 'static {
    /// Create a new `H2ClientTokioTransport` that will use the given `Io` for its underlying raw
    /// IO needs.
    fn new(io: T) -> H2ClientTokioTransport<T> {
        let (read, write) = io.split();
        H2ClientTokioTransport {
            sender: FrameSender::new(write),
            receiver: FrameReceiver::new(read),
            conn: ClientConnection::with_connection(
                HttpConnection::new(HttpScheme::Http),
                DefaultSessionState::<ClientMarker, _>::new()),
            ready_responses: Vec::new(),
            h2stream_to_tokio_request: HashMap::new(),
            tokio_request_to_h2stream: HashMap::new(),
        }
    }

    /// Kicks off a new HTTP request.
    ///
    /// It will set up the HTTP/2 session state appropriately (start tracking a new stream)
    /// and queue up the required HTTP/2 frames onto the connection.
    ///
    /// Also starts tracking the mapping between the Tokio request ID (`request_id`) and the HTTP/2
    /// stream ID that it ends up getting assigned to.
    fn start_request(&mut self, request_id: u64, headers: Vec<StaticHeader>, has_body: bool) {
        let request = self.prepare_request(headers, has_body);

        // Start the request, obtaining the h2 stream ID.
        let stream_id = self.conn.start_request(request, &mut self.sender)
            .ok()
            .expect("queuing a send should work");

        // The ID has been assigned to the stream, so attach it to the stream instance too.
        // TODO(mlalic): The `solicit::Stream` trait should grow an `on_id_assigned` method which
        //               would be called by the session (i.e. the `ClientConnection` in this case).
        //               Indeed, this is slightly awkward...
        self.conn.state.get_stream_mut(stream_id).expect("stream _just_ created").stream_id = Some(stream_id);

        // Now that the h2 request has started, we can keep the mapping of h2 stream ID to the
        // Tokio request ID, so that when the response starts coming in, we can figure out which
        // Tokio request it belongs to...
        debug!("started new request; tokio request={}, h2 stream id={}", request_id, stream_id);
        self.h2stream_to_tokio_request.insert(stream_id, request_id);
        self.tokio_request_to_h2stream.insert(request_id, stream_id);
    }

    /// Prepares a new RequestStream with the given headers. If the request won't have any body, it
    /// immediately closes the stream on the local end to ensure that the peer doesn't expect any
    /// data to come in on the stream.
    fn prepare_request(&mut self, headers: Vec<StaticHeader>, has_body: bool)
            -> RequestStream<'static, 'static, DefaultStream> {
        let mut stream = DefaultStream::new();
        if !has_body {
            stream.close_local();
        }

        RequestStream {
            stream: stream,
            headers: headers,
        }
    }

    /// Handles all frames currently found in the in buffer. After this completes, the buffer will
    /// no longer contain these frames and they will have been seen by the h2 connection, with all
    /// of their effects being reported to the h2 session.
    fn handle_new_frames(&mut self) {
        // We have new data. Let's try parsing and handling as many h2
        // frames as we can!
        while let Some(bytes_to_discard) = self.handle_next_frame() {
            // So far, the frame wasn't copied out of the original input buffer.
            // Now, we'll simply discard from the input buffer...
            self.receiver.discard_frame(bytes_to_discard);
        }
    }

    /// Handles the next frame in the in buffer (if any) and returns its size in bytes. These bytes
    /// can now safely be discarded from the in buffer, as they have been processed by the h2
    /// connection.
    fn handle_next_frame(&mut self) -> Option<usize> {
        match self.receiver.get_next_frame() {
            None => None,
            Some(mut frame_container) => {
                // Give the frame_container to the conn...
                self.conn
                    .handle_next_frame(&mut frame_container, &mut self.sender)
                    .expect("fixme: handle h2 protocol errors gracefully");

                Some(frame_container.len())
            },
        }
    }

    /// Inspects all closed streams and queues up responses that are ready onto the
    /// `ready_responses` queue of `HttpResponseHeaders` that the `Stream` impl will yield.
    fn handle_closed_streams(&mut self) {
        let done = self.conn.state.get_closed();
        for stream in done {
            let stream_id = stream.stream_id.expect("stream is closed");
            let headers = stream.headers.expect("stream is closed");

            let &request_id = self.h2stream_to_tokio_request
                .get(&stream_id)
                .expect("stream must match a known request");

            // NOTE: For now, all responses effectively do not look like streamed responses. Only
            // when the full response is received, do we notify Tokio.
            self.ready_responses.push((request_id, HttpResponseHeaders {
                headers: headers,
                body: stream.body,
            }));
        }
    }

    /// Try to read more data off the socket and handle any HTTP/2 frames that we might
    /// successfully obtain.
    fn try_read_more(&mut self) -> io::Result<()> {
        let total_read = self.receiver.try_read()?;

        if total_read > 0 {
            self.handle_new_frames();

            // After processing frames, let's see if there are any streams that have been completed
            // as a result...
            self.handle_closed_streams();

            // Make sure to issue a write for anything that might have been queued up
            // during the processing of the frames...
            self.sender.try_write()?;
        }

        Ok(())
    }

    /// Dequeue the next response frame off the `ready_responses` queue. As a `Stream` can only
    /// yield a frame at a time, while we can resolve multiple streams (i.e. requests) in the same
    /// stream poll, we need to keep a queue of frames that the `Stream` can yield.
    fn get_next_response_frame(&mut self) -> Option<TokioResponseFrame> {
        if self.ready_responses.len() > 0 {
            let (request_id, response) = self.ready_responses.remove(0);

            Some(Frame::Message {
                id: request_id,
                message: response,
                body: false,
                solo: false,
            })
        } else {
            None
        }
    }

    /// Add a body chunk to the request with the given Tokio ID.
    ///
    /// Currently, we assume that each request will contain only a single body chunk.
    fn add_body_chunk(&mut self, id: u64, chunk: Option<HttpRequestBody>) {
        let stream_id =
            self.tokio_request_to_h2stream
                .get(&id)
                .expect("an in-flight request needs to have an active h2 stream");

        match self.conn.state.get_stream_mut(*stream_id) {
            Some(mut stream) => {
                match chunk {
                    Some(HttpRequestBody { body }) => {
                        trace!("set data for a request stream {}", *stream_id);
                        stream.set_full_data(body);
                    },
                    None => {
                        trace!("no more data for stream {}", *stream_id);
                    },
                };
            },
            None => {},
        };
    }

    /// Attempts to queue up more HTTP/2 frames onto the `sender`.
    fn try_write_next_data(&mut self) -> HttpResult<bool> {
        self.conn.send_next_data(&mut self.sender).map(|res| {
            match res {
                SendStatus::Sent => true,
                SendStatus::Nothing => false,
            }
        })
    }

    /// Try to push out some request body data onto the underlying `Io`.
    fn send_request_data(&mut self) -> Poll<(), io::Error> {
        if !self.has_pending_request_data() {
            // No more pending request data -- we're done sending all requests.
            return Ok(Async::Ready(()));
        }

        trace!("preparing a data frame");
        let has_data = self.try_write_next_data().expect("fixme: Handle protocol failure");
        if has_data {
            debug!("queued up a new data frame");

            if self.sender.try_write()? {
                trace!("wrote a full data frame without blocking");
                // HACK!? Yield to the executor, but make sure we're called back asap...
                // Ensures that we don't simply end up writing a whole lot of request
                // data (bodies) while ignoring to appropriately (timely) handle other
                // aspects of h2 communication (applying settings, sendings acks, initiating
                // _new_ requests, ping/pong, etc...).
                let task = task::park();
                task.unpark();
                Ok(Async::NotReady)
            } else {
                // Did not manage to write the entire new frame without blocking.
                // We'll get rescheduled when the socket unblocks.
                Ok(Async::NotReady)
            }
        } else {
            trace!("no stream data ready");
            // If we didn't manage to prepare a data frame, while there were still open
            // streams, it means that the stream didn't have the data ready for writing.
            // In other words, we've managed to write all Tokio requests -- i.e. anything
            // that passed through `start_send`. When there's another piece of the full
            // HTTP request body ready, we'll get it through `start_send`.
            Ok(Async::Ready(()))
        }
    }

    /// Checks whether any active h2 stream still has data that needs to be sent out to the server.
    /// Returns `true` if there is such a stream.
    ///
    /// This is done by simply checking whether all streams have transitioned into a locally closed
    /// state, which indicates that we're done transmitting from our end.
    fn has_pending_request_data(&mut self) -> bool {
        self.conn.state.iter().any(|(_id, stream)| {
            !stream.is_closed_local()
        })
    }
}

impl<T> Stream for H2ClientTokioTransport<T> where T: Io + 'static {
    type Item = Frame<HttpResponseHeaders, HttpResponseBody, io::Error>;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        trace!("polling read");

        // First, try to see if there's anything more that we can read off the socket already...
        self.try_read_more()?;

        // Now return the first response that we have ready, if any.
        // TODO: Handle eof.
        match self.get_next_response_frame() {
            None => Ok(Async::NotReady),
            Some(tokio_frame) => Ok(Async::Ready(Some(tokio_frame))),
        }
    }
}

impl<T> Sink for H2ClientTokioTransport<T> where T: Io + 'static {
    type SinkItem = Frame<HttpRequestHeaders, HttpRequestBody, io::Error>;
    type SinkError = io::Error;

    fn start_send(&mut self,
                  item: Self::SinkItem)
                  -> StartSend<Self::SinkItem, Self::SinkError> {
        match item {
            Frame::Message { id, body: has_body, message: HttpRequestHeaders { headers }, .. } => {
                debug!("start new request id={}, body={}", id, has_body);
                trace!("  headers={:?}", headers);

                self.start_request(id, headers, has_body);
            },
            Frame::Body { id, chunk } => {
                debug!("add body chunk for request id={}", id);
                self.add_body_chunk(id, chunk);
            },
            _ => {},
        }

        Ok(AsyncSink::Ready)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        trace!("poll all requests sent?");

        // Make sure to trigger a frame flush ...
        if self.sender.try_write()? {
            // If sending everything that was queued so far worked, let's see if we can queue up
            // some data frames, if there are streams that still need to send some.
            self.send_request_data()
        } else {
            // We didn't manage to write everything from our out buffer without blocking.
            // We'll get woken up when writing to the socket is possible again.
            Ok(Async::NotReady)
        }
    }
}

impl<ReadBody, T> Transport<ReadBody> for H2ClientTokioTransport<T> where T: Io + 'static {
    fn tick(&mut self) {
        trace!("TokioTransport TICKING");
    }
}

/// A unit struct that serves to implement the `ClientProto` Tokio trait, which hooks up a
/// raw `Io` to the `H2ClientTokioTransport`.
///
/// This is _almost_ trivial, except it also is required to do protocol negotiation/initialization.
///
/// For cleartext HTTP/2, this means simply sending out the client preface bytes, for which
/// `solicit` provides a helper.
///
/// The transport is resolved only once the preface write is complete, as only after this can the
/// `solicit` `ClientConnection` take over management of the socket: once the HTTP/2 frames start
/// flowing through.
pub struct H2ClientTokioProto;

impl<T> ClientProto<T> for H2ClientTokioProto where T: 'static + Io {
    type Request = HttpRequestHeaders;
    type RequestBody = HttpRequestBody;
    type Response = HttpResponseHeaders;
    type ResponseBody = HttpResponseBody;
    type Error = io::Error;
    type Transport = H2ClientTokioTransport<T>;
    type BindTransport = Box<Future<Item=Self::Transport, Error=io::Error>>;

    fn bind_transport(&self, io: T) -> Self::BindTransport {
        let mut buf = io::Cursor::new(vec![]);
        client::write_preface(&mut buf).expect("writing to an in-memory buffer should not fail");
        let buf = buf.into_inner();

        Box::new(tokio_io::write_all(io, buf).and_then(|(io, _buf)| {
            debug!("client preface write complete");
            future::ok(H2ClientTokioTransport::new(io))
        }))
    }
}

