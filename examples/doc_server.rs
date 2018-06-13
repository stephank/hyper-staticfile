extern crate futures;
extern crate http;
extern crate hyper;
extern crate hyper_staticfile;

// This example serves the docs from target/doc/hyper_staticfile at /doc/
//
// Run `cargo doc && cargo run --example doc_server`, then
// point your browser to http://localhost:3000/

use std::path::Path;

use futures::{Future, future};

use http::{StatusCode, header};
use http::response::Builder as ResponseBuilder;
use hyper::Body;
use hyper_staticfile::Static;

type Request = http::Request<Body>;
type Response = http::Response<Body>;
type ResponseFuture = Box<Future<Item=Response, Error=String> + Send>;

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
    type Error = String;
    type Future = ResponseFuture;

    fn call(&mut self, req: Request) -> ResponseFuture {
        if req.uri().path() == "/" {
            Box::new(future::result(
                ResponseBuilder::new()
                    .status(StatusCode::MOVED_PERMANENTLY)
                    .header(header::LOCATION, "/hyper_staticfile/")
                    .body(Body::empty())
                    .map_err(|e| e.to_string())
            ))
        } else {
            self.static_.call(req)
        }
    }
}

fn main() {
    let addr = ([127, 0, 0, 1], 3000).into();
    let server = hyper::Server::bind(&addr)
        .serve(|| future::ok::<_, String>(MainService::new()))
        .map_err(|e| eprintln!("server error: {}", e));
    eprintln!("Doc server running on http://{}/", addr);
    hyper::rt::run(server);
}
