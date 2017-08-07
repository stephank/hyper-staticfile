extern crate futures;
extern crate hyper;
extern crate hyper_staticfile;
extern crate tokio_core;

// This example serves the docs from target/doc/staticfile at /doc/
//
// Run `cargo doc && cargo run --example doc_server`, then
// point your browser to http://localhost:3000/

use futures::{Future, BoxFuture, Stream, future};
use hyper::server::{Http, Request, Response, Service};
use hyper_staticfile::Static;
use std::path::Path;
use tokio_core::reactor::{Core, Handle};
use tokio_core::net::{TcpListener};

struct MainService {
    static_: Static,
}

impl MainService {
    fn new(handle: &Handle) -> MainService {
        MainService {
            static_: Static::new(handle, Path::new("target/doc/")),
        }
    }
}

impl Service for MainService {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn call(&self, req: Request) -> Self::Future {
        if req.path() == "/" {
            let res = Response::new()
                .with_status(hyper::StatusCode::MovedPermanently)
                .with_header(hyper::header::Location::new("/hyper_staticfile/"));
            future::ok(res).boxed()
        } else {
            Service::call(&self.static_, req)
        }
    }
}

fn main() {
    let mut core = Core::new().unwrap();
    let handle = core.handle();

    let addr = "127.0.0.1:3000".parse().unwrap();
    let listener = TcpListener::bind(&addr, &handle).unwrap();

    let http = Http::new();
    let server = listener.incoming().for_each(|(sock, addr)| {
        let s = MainService::new(&handle);
        http.bind_connection(&handle, sock, addr, s);
        Ok(())
    });

    println!("Doc server running on http://localhost:3000/");
    core.run(server).unwrap();
}
