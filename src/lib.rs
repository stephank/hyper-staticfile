#![crate_name = "hyper_staticfile"]
#![deny(missing_docs)]
#![deny(warnings)]

//! Static file-serving for [Hyper 0.12](https://github.com/hyperium/hyper).

extern crate chrono;
extern crate futures;
extern crate http;
extern crate hyper;
extern crate tokio;
extern crate url;

mod requested_path;
mod static_service;

pub use static_service::Static;
