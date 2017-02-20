//! Exports implementations of HTTP/2 Connectors -- types that can perform protocol negotiation
//! to ensure that the peers agree on using http/2.
//!
//! The two types are `CleartextConnector` and `TlsConnector`. The former simply assumes that
//! cleartext TCP can be used for HTTP/2. The latter performs ALPN negotiation over TLS and
//! only succeeds if the server agrees to use `h2`.
//!
//! Also exports the `H2ConnectorParams` type, which needs to be used by other `Service`s, that
//! want to serve as protocol negotiators.

use client::tls::TlsH2Stream;

use std::io::{self};

use tokio_core::io::{Io};
use tokio_service::Service;
use futures::future::{self, Future};

/// A `Service` impl that can serve as an http/2 connector. It establishes a new TLS-encrypted
/// connection over the given socket, while performing ALPN and ensuring that the server accepts
/// the use of http/2 on the application layer.
pub struct TlsConnector<I> where I: 'static + Io {
    _phantom: ::std::marker::PhantomData<I>,
}

impl<I> TlsConnector<I> where I: 'static + Io {
    pub fn new() -> TlsConnector<I> {
        TlsConnector {
            _phantom: ::std::marker::PhantomData,
        }
    }
}

impl<I> Service for TlsConnector<I> where I: 'static + Io {
    type Request = H2ConnectorParams<I>;
    type Response = TlsH2Stream<I>;
    type Error = io::Error;
    type Future = Box<Future<Item=Self::Response, Error=Self::Error>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        use client::tls::connect_async;

        // Negotiate the application protocol using ALPN (and initialize the TLS session).
        let conn = connect_async(&req.authority, req.io);
        let transport = conn
            .and_then(|io| {
                trace!("ALPN complete");
                // Make sure ALPN yielded the correct protocol
                future::result(TlsH2Stream::new(io))
            });

        Box::new(transport)
    }
}

/// The parameters provided to the http/2 connector `Service`. Provides the authority that the
/// client wants to communicate to and the raw socket.
pub struct H2ConnectorParams<I> where I: 'static + Io {
    pub authority: String,
    pub io: I,
}

impl<I> H2ConnectorParams<I> where I: 'static + Io {
    pub fn new<S: Into<String>>(authority: S, io: I) -> H2ConnectorParams<I> {
        H2ConnectorParams {
            authority: authority.into(),
            io: io,
        }
    }
}

/// Assumes that HTTP/2 can be used over the cleartext TCP socket and simply returns the same
/// socket as what it received without touching it.
pub struct CleartextConnector<I> where I: 'static + Io {
    _phantom: ::std::marker::PhantomData<I>,
}

impl<I> CleartextConnector<I> where I: 'static + Io {
    pub fn new() -> CleartextConnector<I> {
        CleartextConnector {
            _phantom: ::std::marker::PhantomData,
        }
    }
}

impl<I> Service for CleartextConnector<I> where I: 'static + Io {
    type Request = H2ConnectorParams<I>;
    type Response = I;
    type Error = io::Error;
    type Future = future::Ok<Self::Response, Self::Error>;

    fn call(&self, req: Self::Request) -> Self::Future {
        future::ok(req.io)
    }
}

