//! The module implements an HTTP/2 client driven by Tokio's async IO.
//!
//! It exports the `H2Client` struct, which exposes the main client API.

use std::io;
use std::fmt;
use std::error::Error;

use solicit::http::{self as http2, StaticHeader};

mod tokio_layer;
mod client_wrapper;

pub use self::client_wrapper::H2Client;

/// An enum of errors that can arise due to the Tokio layer becoming out-of-sync from the http2
/// session state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TokioSyncError {
    /// An error that would arise if we got a data chunk from Tokio for a request whose body was
    /// previously marked as complete.
    DataChunkAfterEndOfBody,
    /// An error that would arise if we got a Tokio request with an ID that doesn't have a matching
    /// HTTP/2 stream, when one would be expected.
    UnmatchedRequestId,
}

impl fmt::Display for TokioSyncError {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "TokioSyncError: {}", self.description())
    }
}

impl Error for TokioSyncError {
    fn description(&self) -> &str {
        match *self {
            TokioSyncError::DataChunkAfterEndOfBody =>
                "received a data chunk for a request previously marked complete",
            TokioSyncError::UnmatchedRequestId =>
                "received a request id that doesn't have a matching h2 stream",
        }
    }
}

/// An enum of errors that can be raised by the http/2 Transport/Protocol.
#[derive(Debug)]
pub enum Http2Error {
    /// An http/2 protocol error
    Protocol(http2::HttpError),
    /// An IO error (except WouldBlock)
    IoError(io::Error),
    /// Errors due to Tokio and the Transport going out of sync.
    TokioSync(TokioSyncError),
}

impl fmt::Display for Http2Error {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(
            fmt,
            "Http2Error::{}: {}",
            match *self {
                Http2Error::Protocol(_) => "Protocol",
                Http2Error::IoError(_) => "IoError",
                Http2Error::TokioSync(_) => "TokioSync",
            },
            self.description())
    }
}

impl Error for Http2Error {
    fn description(&self) -> &str {
        match *self {
            Http2Error::Protocol(ref err) => err.description(),
            Http2Error::IoError(ref err) => err.description(),
            Http2Error::TokioSync(ref err) => err.description(),
        }
    }
}

impl From<http2::HttpError> for Http2Error {
    fn from(err: http2::HttpError) -> Http2Error {
        Http2Error::Protocol(err)
    }
}

impl From<io::Error> for Http2Error {
    fn from(err: io::Error) -> Http2Error {
        Http2Error::IoError(err)
    }
}

// Tokio needs us to return `io::Error`s from the Transport (and its associated Sink + Stream
// impls). So, we'll use `Http2Error` internally and using this impl finally convert back to
// `io::Error` at the boundary, while still keeping any http2 protocol errors as
// `ErrorKind::Other`.
//
// Hopefully, Tokio will lift this limitation and use something like requiring the used error to
// impl `From<io::Error>` at which point it won't be necessary to wrap/box the http2 error.
impl From<Http2Error> for io::Error {
    fn from(err: Http2Error) -> io::Error {
        match err {
            Http2Error::Protocol(err) => io::Error::new(io::ErrorKind::Other, err),
            Http2Error::IoError(err) => err,
            Http2Error::TokioSync(err) => io::Error::new(io::ErrorKind::Other, err),
        }
    }
}

/// A struct representing the headers used to start a new HTTP request.
///
/// Theoretically, this same struct could be used regardless whether the underlying protocol is
/// h2 or h11.
#[derive(Debug)]
pub struct HttpRequestHeaders {
    headers: Vec<StaticHeader>,
}

impl HttpRequestHeaders {
    pub fn new() -> HttpRequestHeaders {
        HttpRequestHeaders {
            headers: Vec::new(),
        }
    }

    pub fn with_headers(headers: Vec<StaticHeader>) -> HttpRequestHeaders {
        HttpRequestHeaders {
            headers: headers,
        }
    }
}

/// Represents a chunk of the body of an HTTP request.
///
/// Currently requires the chunk to be owned.
#[derive(Debug)]
pub struct HttpRequestBody {
    body: Vec<u8>,
}

impl HttpRequestBody {
    /// Create a new `HttpRequestBody` that will contain the bytes in the given `Vec`.
    pub fn new(body: Vec<u8>) -> HttpRequestBody {
        HttpRequestBody {
            body: body,
        }
    }
}

/// The response to an HTTP request.
///
/// Simply carries the response headers.
#[derive(Debug)]
pub struct HttpResponseHeaders {
    pub headers: Vec<StaticHeader>,
}

/// A chunk of the response body.
#[derive(Debug)]
pub struct HttpResponseBody {
    pub body: Vec<u8>,
}

/// The full response, including both all the headers (including pseudo-headers) and the full body.
#[derive(Debug)]
pub struct HttpResponse {
    pub headers: Vec<StaticHeader>,
    pub body: Vec<u8>,
}
