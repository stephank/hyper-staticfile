extern crate futures;
extern crate http;
extern crate hyper;
extern crate hyper_staticfile;

// This example serves the docs from `target/doc/`.
//
// Run `cargo doc && cargo run --example doc_server`, then
// point your browser to http://localhost:3000/

use futures::{Async::*, Future, Poll, future};
use http::response::Builder as ResponseBuilder;
use http::{Request, Response, StatusCode, header};
use hyper::Body;
use hyper_staticfile::{Static, StaticFuture};
use std::path::Path;
use std::io::Error;

/// Future returned from `MainService`.
enum MainFuture {
    Root,
    Static(StaticFuture<Body>),
}

impl Future for MainFuture {
    type Item = Response<Body>;
    type Error = Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match *self {
            MainFuture::Root => {
                let res = ResponseBuilder::new()
                    .status(StatusCode::MOVED_PERMANENTLY)
                    .header(header::LOCATION, "/hyper_staticfile/")
                    .body(Body::empty())
                    .expect("unable to build response");
                Ok(Ready(res))
            },
            MainFuture::Static(ref mut future) => {
                future.poll()
            }
        }
    }
}

/// Hyper `Service` implementation that serves all requests.
struct MainService {
    static_: Static,
}

impl MainService {
    fn new() -> MainService {
        MainService {
            static_: Static::new(Path::new("target/doc/")),
        }
    }
}

impl hyper::service::Service for MainService {
    type ReqBody = Body;
    type ResBody = Body;
    type Error = Error;
    type Future = MainFuture;

    fn call(&mut self, req: Request<Body>) -> MainFuture {
        if req.uri().path() == "/" {
            MainFuture::Root
        } else {
            MainFuture::Static(self.static_.serve(req))
        }
    }
}

/// Application entry point.
fn main() {
    let addr = ([127, 0, 0, 1], 3000).into();
    let server = hyper::Server::bind(&addr)
        .serve(|| future::ok::<_, Error>(MainService::new()))
        .map_err(|e| eprintln!("server error: {}", e));
    eprintln!("Doc server running on http://{}/", addr);
    hyper::rt::run(server);
}
