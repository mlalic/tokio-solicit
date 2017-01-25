# tokio_solicit

An async HTTP/2 client, based on [Tokio](https://tokio.rs/) and [solicit](https://github.com/mlalic/solicit).

# Example

```rust
extern crate env_logger;

extern crate tokio_core;
extern crate futures;
extern crate tokio_solicit;

use std::str;

use futures::{Future};
use tokio_core::reactor::{Core};

use tokio_solicit::client::H2Client;


fn main() {
    env_logger::init().expect("logger init is required");

    // Prepare the Tokio event loop that will run the request to completion.
    let mut core = Core::new().expect("event loop required");
    let handle = core.handle();

    // Obtain a `SocketAddr` in any supported way...
    let addr = "127.0.0.1:8080".parse().expect("valid IP address");

    // Prepare the http/2 client, providing it additionally the authority that it
    // is to communicate to (aka Host name). This returns a future that resolves to
    // an `H2Client` instance.
    let future_client = H2Client::connect("localhost", &addr, &handle);

    let future_response = future_client.and_then(|mut client| {
        // ...once that is resolved, send out a couple of requests.
        println!("Connection established.");

        let get = client.get(b"/get");
        let post = client.post(b"/post", b"Hello, world!".to_vec());

        // ...and wait for both to complete.
        Future::join(get, post)
    }).map(|(get_response, post_response)| {
        // ...before extracting the response bodies into the result.
        // (Recklessly assume it's utf-8!)
        let get_res: String = str::from_utf8(&get_response.body).unwrap().into();
        let post_res: String = str::from_utf8(&post_response.body).unwrap().into();
        (get_res, post_res)
    });

    // No code up to here actually performed any IO. Now, let's spin up the event loop
    // and wait for the future we built up to resolve to the two response bodies.
    let res = core.run(future_response).expect("responses!");

    println!("{:?}", res);
}

```

# Next steps

Currently this only supports cleartext HTTP/2 connections, which are far and few in the
wild. For a true TLS implementation, we'd need a true async IO ALPN implementation (ALPN
is the only supported way of negotiating that a TLS socket connection will use HTTP/2).

Secondly, the client currently resolves to the entire response (both the response headers
and the full body). Instead, it should resolve to response headers and a `Stream` of body
chunks. There's nothing inherently blocking this from being implemented.

# License

The project is published under the terms of the MIT license.
