use std::path::Path;

use hyper_staticfile::Static;

use hyper_util::rt::TokioIo;

// This test currently only demonstrates that a `Static` instance can be used
// as a hyper service directly.
#[tokio::test]
async fn test_usable_as_hyper_service() {
    let static_ = Static::new(Path::new("target/doc/"));

    let (stream, _) = tokio::io::duplex(2);
    let fut =
        hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), static_);

    // It's enough to show that this builds, so no need to execute anything.
    drop(fut);
}
