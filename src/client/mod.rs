//! The module implements an HTTP/2 client driven by Tokio's async IO.
//!
//! It exports the `H2Client` struct, which exposes the main client API.

use solicit::http::{StaticHeader};

mod tokio_layer;
mod client_wrapper;

pub use self::client_wrapper::H2Client;

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
