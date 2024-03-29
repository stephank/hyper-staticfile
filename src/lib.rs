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
//! // Instance of `Static` containing configuration. Can be cheaply cloned.
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
//! The `Static` type is a simple wrapper for `Resolver` and `ResponseBuilder`. You can achieve the
//! same by doing something similar to the following:
//!
//! ```rust
//! use std::path::Path;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create a resolver. This can be cheaply cloned.
//!     let resolver = hyper_staticfile::Resolver::new("my/doc/root/");
//!
//!     // A dummy request, but normally obtained from Hyper.
//!     let request = http::Request::get("/foo/bar.txt")
//!         .body(())
//!         .unwrap();
//!
//!     // First, resolve the request. Returns a future for a `ResolveResult`.
//!     let result = resolver.resolve_request(&request).await.unwrap();
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
//! The `resolve_request` method tries to find the file in the document root, and returns a future
//! for the `ResolveResult` enum, which determines what kind of response should be sent. The
//! `ResponseBuilder` is then used to create a default response. It holds some settings, and can be
//! constructed using the builder pattern.
//!
//! It's useful to sit between these two steps to implement custom 404 pages, for example. Your
//! custom logic can override specific cases of `ResolveResult`, and fall back to the default
//! behavior using `ResponseBuilder` if necessary.

mod body;
mod resolve;
mod response_builder;
mod service;

/// Lower level utilities.
pub mod util;
/// Types to implement a custom (virtual) filesystem to serve files from.
pub mod vfs;

pub use crate::body::Body;
pub use crate::resolve::*;
pub use crate::response_builder::*;
pub use crate::service::*;
