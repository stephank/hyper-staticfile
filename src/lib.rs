#![crate_name = "hyper_staticfile"]
#![deny(missing_docs)]

//! Static file-serving for [Hyper 1.0](https://github.com/hyperium/hyper).
//!
//! This library exports a high-level interface `Static` for simple file-serving, and lower-level
//! interfaces for more control over responses.
//!
//! ## Basic usage
//!
//! The `Static` type is essentially a struct containing some settings, and a `serve` method to
//! handle the request. It follows the builder pattern, and also implements the `hyper::Service`
//! trait. It can be used as:
//!
//! ```rust
//! // Instance of `Static` containing configuration.
//! let static_ = hyper_staticfile::Static::new("my/doc/root/");
//!
//! // A dummy request, but normally obtained from Hyper.
//! let request = http::Request::get("/foo/bar.txt")
//!     .body(())
//!     .unwrap();
//!
//! // Serve the request. Returns a future for a `hyper::Response`.
//! let response_future = static_.serve(request);
//! ```
//!
//! Typically, you'd store the `Static` instance somewhere, such as in your own `hyper::Service`
//! implementation.
//!
//! ## Advanced usage
//!
//! The `Static` type is a simple wrapper for `resolve` and `ResponseBuilder`. You can achieve the
//! same by doing something similar to the following:
//!
//! ```rust
//! use std::path::Path;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Document root path.
//!     let root = Path::new("my/doc/root/");
//!
//!     // A dummy request, but normally obtained from Hyper.
//!     let request = http::Request::get("/foo/bar.txt")
//!         .body(())
//!         .unwrap();
//!
//!     // First, resolve the request. Returns a future for a `ResolveResult`.
//!     let result = hyper_staticfile::resolve(&root, &request)
//!         .await
//!         .unwrap();
//!
//!     // Then, build a response based on the result.
//!     // The `ResponseBuilder` is typically a short-lived, per-request instance.
//!     let response = hyper_staticfile::ResponseBuilder::new()
//!         .request(&request)
//!         .build(result)
//!         .unwrap();
//! }
//! ```
//!
//! The `resolve` function tries to find the file in the root, and returns a future for the
//! `ResolveResult` enum, which determines what kind of response should be sent. The
//! `ResponseBuilder` is then used to create a default response. It holds some settings, and can be
//! constructed using the builder pattern.
//!
//! It's useful to sit between these two steps to implement custom 404 pages, for example. Your
//! custom logic can override specific cases of `ResolveResult`, and fall back to the default
//! behavior using `ResponseBuilder` if necessary.
//!
//! The `ResponseBuilder` in turn uses `FileResponseBuilder` to serve files that are found. The
//! `FileResponseBuilder` can also be used directly if you have an existing open `tokio::fs::File`
//! and want to serve it. It takes care of basic headers, 'not modified' responses, and streaming
//! the file in the body.
//!
//! Finally, there's `FileBytesStream`, which is used by `FileResponseBuilder` to stream the file.
//! This is a struct wrapping a `tokio::fs::File` and implementing a `futures::Stream` that
//! produces `Bytes`s. It can be used for streaming a file in custom response.

mod resolve;
mod response_builder;
mod service;
mod util;

pub use crate::resolve::*;
pub use crate::response_builder::*;
pub use crate::service::*;
pub use crate::util::{Body, FileBytesStream, FileResponseBuilder};
