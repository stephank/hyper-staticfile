#![crate_name = "hyper_staticfile"]
#![deny(missing_docs)]
#![deny(warnings)]

//! Static file-serving for [Hyper 0.12](https://github.com/hyperium/hyper).

extern crate chrono;
#[macro_use]
extern crate futures;
extern crate http;
extern crate hyper;
extern crate tokio;
extern crate url;

mod resolve;
mod response_builder;
mod service;
mod util;

pub use resolve::*;
pub use response_builder::*;
pub use service::*;
