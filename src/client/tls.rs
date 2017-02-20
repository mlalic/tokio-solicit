//! Contains an implementation of TLS-related utilities.
//! 
//! `connect_async` - a function that performs the TLS handshake on a fresh socket, including
//! ALPN for `h2`.
//! 
//! `TlsH2Stream` - a struct that can be used to wrap an `SslStream`, if it has successfully
//! negotiated to use `h2`. Implements `tokio_core::io::Io`, meaning that it can be seamlessly
//! used by Tokio.
use std::io::{self};

use openssl::ssl::{SslConnectorBuilder, SslConnector, SslMethod};
use tokio_openssl::{SslConnectorExt, SslStream};

use tokio_core::io::{Io};
use futures::{Async, Future};
use futures::future;

/// Performs the TLS handshake on the given socket, assuming it's freshly connected. Returns
/// a (boxed) future that resolves to an `SslStream` with initialized TLS. It also negotiates
/// the application protocol using ALPN to be `h2`.
pub fn connect_async<I>(authority: &str,
                        io: I)
                        -> Box<Future<Item=SslStream<I>, Error=io::Error>>
                        where I: 'static + Io {

    let connector = match make_connector() {
        Err(e) => return Box::new(future::err(e)),
        Ok(connector) => connector,
    };

    let conn = connector.connect_async(authority, io);
    let conn = conn.map_err(|_e| io::Error::from(io::ErrorKind::NotConnected));
    Box::new(conn)
}

/// A helper function to create an `SslConnector` that is configured to do ALPN.
fn make_connector() -> Result<SslConnector, io::Error> {
    let mut builder = SslConnectorBuilder::new(SslMethod::tls())?;
    builder.builder_mut().set_alpn_protocols(&[b"h2"])?;

    Ok(builder.build())
}

/// A TLS stream after negotiating to use http2 as the application protocol over ALPN.
pub struct TlsH2Stream<I: Io> {
    inner: SslStream<I>,
}

impl<I> TlsH2Stream<I> where I: Io {
    pub fn new(inner: SslStream<I>) -> Result<TlsH2Stream<I>, io::Error> {
        let is_h2 = {
            let protocol = inner.get_ref().ssl().selected_alpn_protocol();
            let protocol = protocol.ok_or(io::Error::from(io::ErrorKind::UnexpectedEof))?;
            trace!("Negotiated application protocol: {:?}", protocol);

            protocol == b"h2"
        };

        if is_h2 {
            Ok(TlsH2Stream {
                inner: inner,
            })
        } else {
            Err(io::Error::from(io::ErrorKind::UnexpectedEof))
        }
    }
}

impl<I> io::Read for TlsH2Stream<I> where I: Io {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<I> io::Write for TlsH2Stream<I> where I: Io {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<I> Io for TlsH2Stream<I> where I: Io {
    #[inline]
    fn poll_read(&mut self) -> Async<()> {
        self.inner.poll_read()
    }

    #[inline]
    fn poll_write(&mut self) -> Async<()> {
        self.inner.poll_write()
    }
}
