# tokio_solicit

An async HTTP/2 client, based on [Tokio](https://tokio.rs/) and [solicit](https://github.com/mlalic/solicit).

# Example

```rust
extern crate env_logger;

extern crate tokio_core;
extern crate futures;
extern crate tokio_solicit;

use std::str;
use std::net::{ToSocketAddrs};

use futures::{Future};
use tokio_core::reactor::{Core};

use tokio_solicit::client::H2Client;


fn main() {
    env_logger::init().expect("logger init is required");

    // Prepare the Tokio event loop that will run the request to completion.
    let mut core = Core::new().expect("event loop required");
    let handle = core.handle();

    // Obtain a `SocketAddr` in any supported way...
    let addr =
        "google.com:443"
            .to_socket_addrs()
            .expect("unable to resolve the domain name")
            .next()
            .expect("no matching ip addresses");

    // Prepare the http/2 client, providing it additionally the authority that it
    // is to communicate to (aka Host name). This returns a future that resolves to
    // an `H2Client` instance...
    let future_client = H2Client::connect("google.com", &addr, &handle);

    let future_response = future_client.and_then(|mut client| {
        // ...once that is resolved, send out the request.
        println!("Connection established.");

        // (We want the future to resolve only once the full body is ready)
        client.get(b"/").into_full_body_response()
    });

    // No code up to here actually performed any IO. Now, let's spin up the event loop
    // and wait for the future we built up to resolve to the response.
    let response = core.run(future_response).expect("unexpected error");

    // Print both the headers and the response body...
    println!("{:?}", response.headers);
    // (Recklessly assume it's utf-8!)
    let body = str::from_utf8(&response.body).unwrap();
    println!("{}", body);
}

```

# Next steps

Currently this only supports cleartext HTTP/2 connections, which are far and few in the
wild. For a true TLS implementation, we'd need a true async IO ALPN implementation (ALPN
is the only supported way of negotiating that a TLS socket connection will use HTTP/2).

# License

The project is published under the terms of the MIT license.
