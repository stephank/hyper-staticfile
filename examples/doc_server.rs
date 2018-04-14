extern crate futures;
extern crate hyper;
extern crate hyper_staticfile;

// This example serves the docs from target/doc/hyper_staticfile at /doc/
//
// Run `cargo doc && cargo run --example doc_server`, then
// point your browser to http://localhost:3000/

use std::path::Path;

use futures::{Future, future};

use hyper::Error;
use hyper::server::{Http, Request, Response, Service};
use hyper_staticfile::Static;

type ResponseFuture = Box<Future<Item=Response, Error=Error>>;

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

impl Service for MainService {
    type Request = Request;
    type Response = Response;
    type Error = Error;
    type Future = ResponseFuture;

    fn call(&self, req: Request) -> Self::Future {
        if req.path() == "/" {
            let res = Response::new()
                .with_status(hyper::StatusCode::MovedPermanently)
                .with_header(hyper::header::Location::new("/hyper_staticfile/"));
            Box::new(future::ok(res))
        } else {
            self.static_.call(req)
        }
    }
}

fn main() {
    let addr = "127.0.0.1:3000".parse().unwrap();
    let server = Http::new().bind(&addr, || Ok(MainService::new())).unwrap();
    println!("Doc server running on http://localhost:3000/");

    server.run().unwrap();
}
