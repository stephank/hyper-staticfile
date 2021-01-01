use futures_util::future;
use hyper::service::make_service_fn;
use hyper_staticfile::Static;
use std::path::Path;

// This test currently only demonstrates that a `Static` instance can be used
// as a hyper service directly.
#[tokio::test]
async fn test_usable_as_hyper_service() {
    let static_ = Static::new(Path::new("target/doc/"));

    let make_service = make_service_fn(|_| {
        let static_ = static_.clone();
        future::ok::<_, hyper::Error>(static_)
    });

    // Bind to port "0" to allow the OS to pick one that's free, avoiding
    // the risk of collisions.
    let addr = ([127, 0, 0, 1], 0).into();
    let server = hyper::server::Server::bind(&addr).serve(make_service);

    // It's enough to show that this builds, so no need to execute anything.
    drop(server);
}
