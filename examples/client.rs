extern crate env_logger;

extern crate tokio_core;
extern crate futures;
extern crate tokio_solicit;

use std::str;
use std::io::{self};

use futures::{Future, Stream};
use futures::future::{self};
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

        // Accumulate the bodies of each request into a single vector, ignoring the
        // headers...
        // Here we do it using the convenience method exposed by the request Future.
        let get = get.into_full_body_response().map(|(_headers, body)| body);

        // In this case, do it "manually" in order to do some more processing for each chunk (for
        // demo purposes).
        let post = post.and_then(|(_, body)| {
            body.fold(Vec::<u8>::new(), |mut vec, chunk| {
                println!("receiving a new chunk of size {}", chunk.body.len());

                vec.extend(chunk.body.into_iter());
                future::ok::<_, io::Error>(vec)
            })
        });

        // ...and yield a future that resolves once both bodies are ready
        Future::join(get, post)
    }).map(|(get_response_body, post_response_body)| {
        // Convert the bodies to a UTF-8 string
        let get_res: String = str::from_utf8(&get_response_body).unwrap().into();
        let post_res: String = str::from_utf8(&post_response_body).unwrap().into();

        // ...and yield a pair of bodies converted to a string.
        (get_res, post_res)
    });

    let res = core.run(future_response).expect("responses!");

    println!("{:?}", res);

    // An additional demo showing how to perform a streaming _request_ (i.e. the body of the
    // request is streamed out to the server).
    do_streaming_request(&mut core);
}

fn do_streaming_request(core: &mut Core) {
    use std::iter;
    use tokio_solicit::client::HttpRequestBody;
    use futures::Sink;

    let handle = core.handle();
    let addr = "127.0.0.1:8080".parse().expect("valid IP address");
    let future_client = H2Client::connect("localhost", &addr, &handle);

    let future_response = future_client.and_then(|mut client| {
        let (post, tx) = client.streaming_request(b"POST", b"/post", iter::empty());
        tx
            .send(Ok(HttpRequestBody::new(b"HELLO ".to_vec())))
            .and_then(|tx| tx.send(Ok(HttpRequestBody::new(b" WORLD".to_vec()))))
            .and_then(|tx| tx.send(Ok(HttpRequestBody::new(b"!".to_vec()))))
            .map_err(|_err| io::Error::from(io::ErrorKind::BrokenPipe))
            .and_then(|_tx| post.into_full_body_response())
    });

    let res = core.run(future_response).expect("response");
    println!("{:?}", res);
}
