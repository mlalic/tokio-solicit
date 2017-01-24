//! The module exposes the `H2Client` struct, which implements a futures-based API for performing
//! HTTP/2 requests, based on Tokio.

use super::{HttpRequestHeaders, HttpRequestBody, HttpResponseHeaders, HttpResponseBody};
use client::tokio_layer::{H2ClientTokioProto};

use std::io::{self};
use std::net::SocketAddr;
use std::iter::{self, IntoIterator};

use futures::{Async, Future, Poll};
use futures::future::{BoxFuture};
use futures::stream::{self};

use tokio_core::reactor::{Handle};
use tokio_proto::{Connect, TcpClient};
use tokio_proto::streaming::{Message, Body};
use tokio_proto::streaming::multiplex::{StreamingMultiplex};
use tokio_proto::util::client_proxy::ClientProxy;

use tokio_service::{Service};

use solicit::http::{Header, StaticHeader};

/// A type alias for the request body stream. For now, we assume that all requests are a
/// single-chunk stream.
type RequestBodyStream = stream::Once<HttpRequestBody, io::Error>;

/// A type alias for the `ClientProxy` that we end up building after an `Io` is bound to a
/// `H2ClientTokioTransport` by `H2ClientTokioProto`.
type TokioClient =
    ClientProxy<
        Message<HttpRequestHeaders, RequestBodyStream>,
        Message<HttpResponseHeaders, Body<HttpResponseBody, io::Error>>,
        io::Error>;

/// A type alias for the future that is produced by the `H2Client`'s various `request` methods.
/// (`request`, `get`, `post`, ...)
type FutureH2Response = BoxFuture<HttpResponseHeaders, io::Error>;

/// A struct that implements a futures-based API for an HTTP/2 client.
pub struct H2Client {
    /// The inner ClientProxy that hooks into the whole Tokio infrastructure.
    inner: TokioClient,
    /// The authority header (nee Host). Specifies the host name that the HTTP requests will
    /// be directed at. This is distinct from the socket address.
    authority: Vec<u8>,
}

impl H2Client {
    /// Creates a new `H2Client` from the given `TokioClient`.
    fn new(inner: TokioClient, authority: Vec<u8>) -> H2Client {
        H2Client {
            inner: inner,
            authority: authority,
        }
    }

    /// Connect to the given socket and yield a new `H2Client` that can be used to send HTTP/2
    /// requests to this socket.
    ///
    /// Returns a future that will resolve to the `H2Client`.
    pub fn connect(authority: &str, socket_addr: &SocketAddr, handle: &Handle) -> H2ClientNew {
        let proto = H2ClientTokioProto;

        let client = TcpClient::<StreamingMultiplex<RequestBodyStream>, _>::new(proto);
        let connect = client.connect(&socket_addr, &handle);

        H2ClientNew::new(connect, authority.as_bytes().to_vec())
    }

    /// Issues a GET request to the server.
    ///
    /// Yields a future that resolves to an `HttpRequestHeaders` struct. This struct will carry
    /// both the response headers, as well as the response body.
    ///
    /// TODO: WTB `-> impl` to avoid boxing this (or manually implementing a Future)
    pub fn get(&mut self, path: &[u8]) -> FutureH2Response {
        self.request(b"GET", path, iter::empty(), None)
    }

    /// Issues a POST request, carrying the given body.
    pub fn post(&mut self, path: &[u8], body: Vec<u8>) -> FutureH2Response {
        self.request(b"POST", path, iter::empty(), Some(body))
    }

    /// Perform a request, providing manually the request method, headers, and body.
    pub fn request<I>(&mut self,
                      method: &[u8],
                      path: &[u8],
                      user_headers: I,
                      body: Option<Vec<u8>>)
                      -> FutureH2Response
                      where I: IntoIterator<Item=StaticHeader> {
        let mut headers = Vec::new();
        headers.extend(vec![
            Header::new(b":method", method.to_vec()),
            Header::new(b":path", path.to_vec()),
            Header::new(b":authority", self.authority.clone()),
            Header::new(b":scheme", b"http"),
        ].into_iter());
        headers.extend(user_headers.into_iter());

        self.request_with_vec(headers, body)
    }

    /// Actually performs the full request. Avoids monomorphizing the entire code, but rather only
    /// the bit that requires the use of the IntoIterator trait, before passing off to this.
    fn request_with_vec(&mut self,
                        headers: Vec<StaticHeader>,
                        body: Option<Vec<u8>>)
                        -> FutureH2Response {

        let request_headers = HttpRequestHeaders::with_headers(headers);
        let tokio_req = match body {
            None => Message::WithoutBody(request_headers),
            Some(body) => {
                let body_stream = stream::once::<_, io::Error>(Ok(HttpRequestBody::new(body)));
                Message::WithBody(request_headers, body_stream)
            },
        };

        let response_future = Service::call(&self.inner, tokio_req).map(|response| {
            debug!("resolved response message");

            match response {
                Message::WithoutBody(resp @ HttpResponseHeaders { .. }) => {
                    // Currently the entire response (including the actual response body)
                    // will be found in the HttpResponseHeaders.
                    resp
                },
                _ => unreachable!("currently all responses are a single blob"),
            }
        });

        response_future.boxed()
    }
}

/// A simple `Future` implementation that resolves once the HTTP/2 client connection is
/// established.
pub struct H2ClientNew {
    /// The future that resolves to a new Tokio ClientProxy.
    inner: Connect<StreamingMultiplex<RequestBodyStream>, H2ClientTokioProto>,
    /// The authority that the new client will send requests to.
    authority: Option<Vec<u8>>,
}

impl H2ClientNew {
    fn new(connect: Connect<StreamingMultiplex<RequestBodyStream>, H2ClientTokioProto>,
           authority: Vec<u8>)
           -> H2ClientNew {
        H2ClientNew {
            inner: connect,
            authority: Some(authority),
        }
    }
}

impl Future for H2ClientNew {
    type Item = H2Client;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        trace!("waiting for client connection...");

        match self.inner.poll() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(client_proxy)) => {
                trace!("client connected");
                let authority = self.authority.take().expect("H2ClientNew future polled again");
                Ok(Async::Ready(H2Client::new(client_proxy, authority)))
            },
            Err(e) => Err(e),
        }
    }
}
