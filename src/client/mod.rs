//! The module implements an HTTP/2 client driven by Tokio's async IO.

use solicit::http::{StaticHeader};

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
/// Currently, the response immedialely carries the entire contents of the body, i.e. the body is
/// not streamed, even though the `tokio_layer` is based on a streaming/multiplexed Tokio protcol.
/// This is a temporary WIP/PoC state...
#[derive(Debug)]
pub struct HttpResponseHeaders {
    pub headers: Vec<StaticHeader>,
    pub body: Vec<u8>,
}

/// A chunk of the response body.
///
/// Currently only used to satisfy the various type bounds in `tokio_layer` with a placeholder
/// type, as the entire response is always "bodiless" from Tokio's perspective (the body is carried
/// in the "headers").
pub struct HttpResponseBody;
