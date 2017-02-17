//! The module exposes the `H2Client` struct, which implements a futures-based API for performing
//! HTTP/2 requests, based on Tokio.

use super::{
    HttpRequestHeaders,
    HttpRequestBody,
    HttpResponseHeaders,
    HttpResponseBody,
    HttpResponse
};
use client::connectors::{CleartextConnector, H2ConnectorParams};
use client::tokio_layer::{H2ClientTokioProto};

use std::io::{self};
use std::net::SocketAddr;
use std::iter::{self, IntoIterator};

use futures::{Async, Future, Poll};
use futures::future::{self, BoxFuture};
use futures::stream::{Stream};
use futures::sync::mpsc;

use tokio_core::reactor::{Handle};
use tokio_core::io::{Io};
use tokio_core::net::TcpStream;
use tokio_service::{Service};
use tokio_proto::{Connect, TcpClient};
use tokio_proto::streaming::{Message, Body};
use tokio_proto::streaming::multiplex::{StreamingMultiplex};
use tokio_proto::util::client_proxy::ClientProxy;

use solicit::http::{Header, StaticHeader};

/// A type alias for the request body stream.
type RequestBodyStream = Body<HttpRequestBody, io::Error>;

/// A type alias for the `ClientProxy` that we end up building after an `Io` is bound to a
/// `H2ClientTokioTransport` by `H2ClientTokioProto`.
type TokioClient =
    ClientProxy<
        Message<HttpRequestHeaders, RequestBodyStream>,
        Message<HttpResponseHeaders, Body<HttpResponseBody, io::Error>>,
        io::Error>;

/// A `futures::Stream` impl that represents the body of the response. The `Future` returned
/// by various `H2Client` methods returns an instance of this type, along with the response
/// headers.
pub struct ResponseBodyStream {
    /// The type simply hides away the Tokio `Body`, which will be returned by Tokio client
    /// Service.
    inner: Body<HttpResponseBody, io::Error>,
}

impl ResponseBodyStream {
    fn new(inner: Body<HttpResponseBody, io::Error>) -> ResponseBodyStream {
        ResponseBodyStream {
            inner: inner,
        }
    }
}

impl Stream for ResponseBodyStream {
    type Item = HttpResponseBody;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.inner.poll()
    }
}

/// A `Future` produced by the `H2Client`'s various `request` methods.
/// (`request`, `get`, `post`, ...)
pub struct FutureH2Response {
    /// Simply wraps a boxed future
    inner: BoxFuture<(HttpResponseHeaders, ResponseBodyStream), io::Error>,
}

impl Future for FutureH2Response {
    type Item = (HttpResponseHeaders, ResponseBodyStream);
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.inner.poll()
    }
}

impl FutureH2Response {
    /// Creates a new `FutureH2Response` wrapping the given boxed future.
    fn new(inner: BoxFuture<(HttpResponseHeaders, ResponseBodyStream), io::Error>)
            -> FutureH2Response {
        FutureH2Response {
            inner: inner,
        }
    }

    /// Consumes the `FutureH2Response` and returns a new `Future` that will resolve once the full
    /// body of the response has become available, with both the response headers and all the body
    /// bytes in a `Vec<u8>` (as an `HttpResponse` instance).
    pub fn into_full_body_response(self) -> BoxFuture<HttpResponse, io::Error> {
        let body_response = self.and_then(|(headers, body_stream)| {
            body_stream
                .fold(Vec::<u8>::new(), |mut vec, chunk| {
                    vec.extend(chunk.body.into_iter());
                    future::ok::<_, io::Error>(vec)
                })
                .map(move |body| {
                    HttpResponse {
                        headers: headers.headers,
                        body: body
                    }
                })
        });

        body_response.boxed()
    }
}

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
    ///
    /// The HTTP/2 connection will be executed in cleartext, over the raw socket.
    pub fn cleartext_connect(authority: &str,
                             socket_addr: &SocketAddr,
                             handle: &Handle)
                             -> H2ClientNew<CleartextConnector<TcpStream>> {

        H2Client::with_connector(
            authority,
            socket_addr,
            handle,
            CleartextConnector::<TcpStream>::new())
    }

    /// Connect to the given socket and yield a new `H2Client` that can be used to send HTTP/2
    /// requests to this socket.
    ///
    /// Returns a future that will resolve to the `H2Client`.
    ///
    /// HTTP/2 connection negotiation needs to be performed by the provided `Connector` instance,
    /// which will be provided the raw socket as soon as the client has connected.
    pub fn with_connector<Connector>(
        authority: &str,
        socket_addr: &SocketAddr,
        handle: &Handle,
        connector: Connector)
        -> H2ClientNew<Connector>
            where Connector: Service<Request=H2ConnectorParams<TcpStream>, Error=io::Error>,
                  Connector::Response: Io {

        let proto = H2ClientTokioProto {
            connector: connector,
            authority: authority.into(),
        };
        let client = TcpClient::<StreamingMultiplex<RequestBodyStream>, _>::new(proto);
        let connect = client.connect(&socket_addr, &handle);

        H2ClientNew::new(connect, authority.as_bytes().to_vec())
    }

    /// Issues a GET request to the server.
    ///
    /// Yields a future that resolves to an `HttpRequestHeaders` struct. This struct will carry
    /// both the response headers, as well as the response body.
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
        let request_headers = self.prepare_headers(method, path, user_headers);
        let tokio_message = match body {
            None => Message::WithoutBody(request_headers),
            Some(body) => {
                let body_stream = Body::from(HttpRequestBody::new(body));
                Message::WithBody(request_headers, body_stream)
            },
        };

        self.request_with_message(tokio_message)
    }

    /// Perform a request, where the method and path are already provided, while the body should be
    /// streamed out by posting body chunks (`HttpRequestBody` instances) onto the returned
    /// channel.
    pub fn streaming_request<I>(&mut self,
                                method: &[u8],
                                path: &[u8],
                                user_headers: I)
                                -> (FutureH2Response, mpsc::Sender<Result<HttpRequestBody, io::Error>>)
                                where I: IntoIterator<Item=StaticHeader> {

        let headers = self.prepare_headers(method, path, user_headers);
        let (tx, body) = Body::pair();

        (self.request_with_message(Message::WithBody(headers, body)), tx)
    }

    /// Actually performs the full request. Avoids monomorphizing the entire code, but rather only
    /// the bit that requires the use of the IntoIterator trait, before passing off to this.
    fn request_with_message(&mut self,
                            message: Message<HttpRequestHeaders, RequestBodyStream>)
                            -> FutureH2Response {
        let response_future = Service::call(&self.inner, message).map(|response| {
            debug!("resolved response message");

            match response {
                Message::WithoutBody(resp @ HttpResponseHeaders { .. }) => {
                    // If there's no body, just yield an empty body stream.
                    (resp, ResponseBodyStream::new(Body::empty()))
                },
                Message::WithBody(resp @ HttpResponseHeaders { .. }, body) => {
                    (resp, ResponseBodyStream::new(body))
                },
            }
        });

        FutureH2Response::new(response_future.boxed())
    }

    /// Creates `HttpRequestHeaders` that will include appropriate pseudo-headers, as well as the
    /// given user-provided extra headers.
    fn prepare_headers<I>(&mut self,
                          method: &[u8],
                          path: &[u8],
                          user_headers: I)
                          -> HttpRequestHeaders
                          where I: IntoIterator<Item=StaticHeader> {
        let mut headers = Vec::new();
        headers.extend(vec![
            Header::new(b":method", method.to_vec()),
            Header::new(b":path", path.to_vec()),
            Header::new(b":authority", self.authority.clone()),
            Header::new(b":scheme", b"http"),
        ].into_iter());
        headers.extend(user_headers.into_iter());

        HttpRequestHeaders::with_headers(headers)
    }
}

/// A simple `Future` implementation that resolves once the HTTP/2 client connection is
/// established.
pub struct H2ClientNew<Connector>
        where Connector: 'static + Service<Request=H2ConnectorParams<TcpStream>, Error=io::Error>,
              Connector::Response: 'static + Io {

    /// The future that resolves to a new Tokio ClientProxy.
    inner: Connect<StreamingMultiplex<RequestBodyStream>, H2ClientTokioProto<Connector>>,

    /// The authority that the new client will send requests to.
    authority: Option<Vec<u8>>,
}

impl<Connector> H2ClientNew<Connector>
        where Connector: 'static + Service<Request=H2ConnectorParams<TcpStream>, Error=io::Error>,
              Connector::Response: 'static + Io {

    fn new(connect: Connect<StreamingMultiplex<RequestBodyStream>, H2ClientTokioProto<Connector>>,
           authority: Vec<u8>)
           -> H2ClientNew<Connector> {
        H2ClientNew {
            inner: connect,
            authority: Some(authority),
        }
    }
}

impl<Connector> Future for H2ClientNew<Connector>
        where Connector: 'static + Service<Request=H2ConnectorParams<TcpStream>, Error=io::Error>,
              Connector::Response: 'static + Io {
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
