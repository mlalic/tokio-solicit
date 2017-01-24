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

    let mut core = Core::new().expect("event loop required");
    let handle = core.handle();

    let addr = "127.0.0.1:8080".parse().expect("valid IP address");
    /*
    use std::net::{ToSocketAddrs};
    let addr =
        "http2bin.org:80"
            .to_socket_addrs()
            .expect("unable to resolve the domain name")
            .next()
            .expect("no matching ip addresses");
    */

    println!("Socket address - {:?}", addr);

    let future_client = H2Client::connect("localhost", &addr, &handle);

    let future_response = future_client.and_then(|mut client| {
        println!("Connection established.");

        let get = client.get(b"/get");
        let post = client.post(b"/post", b"Hello, world!".to_vec());

        Future::join(get, post)
    }).map(|(get_response, post_response)| {
        let get_res: String = str::from_utf8(&get_response.body).unwrap().into();
        let post_res: String = str::from_utf8(&post_response.body).unwrap().into();
        (get_res, post_res)
    });

    let res = core.run(future_response).expect("responses!");

    println!("{:?}", res);
}
