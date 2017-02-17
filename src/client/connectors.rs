//! Exports implementations of HTTP/2 Connectors -- types that can perform protocol negotiation
//! to ensure that the peers agree on using http/2.

use std::io::{self};

use tokio_core::io::{Io};
use tokio_service::Service;
use futures::future::{self};

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

