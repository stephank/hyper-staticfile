extern crate futures;
extern crate hyper;
extern crate hyper_staticfile;
extern crate tempdir;
extern crate tokio_core;

use std::{fs, str};
use std::io::Write;
use std::time::{SystemTime, Duration};
use futures::{Future, Stream, future};
use hyper::{Method, StatusCode, Error, header};
use hyper::server::{Service, Request, Response};
use hyper_staticfile::Static;
use tempdir::TempDir;
use tokio_core::reactor::Core;

type EmptyFuture = Box<Future<Item=(), Error=()>>;
type ResponseFuture = Box<Future<Item=Response, Error=Error>>;

struct Harness {
    static_: Static,
}
impl Harness {
    fn run<F>(files: Vec<(&str, &str)>, f: F)
            where F: FnOnce(Harness) -> EmptyFuture {
        let dir = TempDir::new("hyper-staticfile-tests").unwrap();
        for (subpath, contents) in files {
            let fullpath = dir.path().join(subpath);
            fs::create_dir_all(fullpath.parent().unwrap())
                .and_then(|_| fs::File::create(fullpath))
                .and_then(|mut file| file.write(contents.as_bytes()))
                .expect("failed to write fixtures");
        }

        let mut core = Core::new().unwrap();
        let handle = core.handle();
        let static_ = Static::new(&handle, dir.path().clone())
            .with_cache_headers(3600);

        core.run(f(Harness { static_ })).expect("failed to run event loop");
    }

    fn request(&self, req: Request) -> ResponseFuture {
        self.static_.call(req)
    }

    fn get(&self, path: &str) -> ResponseFuture {
        self.request(Request::new(Method::Get, path.parse().unwrap()))
    }
}

#[test]
fn serves_non_default_file_from_absolute_root_path() {
    Harness::run(vec![
        ("file1.html", "this is file1")
    ], |harness| {
        let f = harness.get("/file1.html")
            .and_then(|res| res.body().concat2())
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is file1");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}

#[test]
fn serves_default_file_from_absolute_root_path() {
    Harness::run(vec![
        ("index.html", "this is index")
    ], |harness| {
        let f = harness.get("/index.html")
            .and_then(|res| res.body().concat2())
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is index");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}

#[test]
fn returns_404_if_file_not_found() {
    Harness::run(vec![], |harness| {
        let f = harness.get("/")
            .and_then(|res|  {
                assert_eq!(res.status(), StatusCode::NotFound);
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}

#[test]
fn redirects_if_trailing_slash_is_missing() {
    Harness::run(vec![
        ("dir/index.html", "this is index"),
    ], |harness| {
        let f = harness.get("/dir")
            .and_then(|res|  {
                assert_eq!(res.status(), StatusCode::MovedPermanently);

                let url: &str = res.headers().get::<header::Location>().unwrap();
                assert_eq!(url, "/dir/");

                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}

#[test]
fn decodes_percent_notation() {
    Harness::run(vec![
        ("has space.html", "file with funky chars")
    ], |harness| {
        let f = harness.get("/has space.html")
            .and_then(|res| res.body().concat2())
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "file with funky chars");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}

#[test]
fn normalizes_path() {
    Harness::run(vec![
        ("index.html", "this is index")
    ], |harness| {
        let f = harness.get("/xxx/../index.html")
            .and_then(|res| res.body().concat2())
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is index");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}

#[test]
fn normalizes_percent_encoded_path() {
    Harness::run(vec![
        ("file1.html", "this is file1")
    ], |harness| {
        let f = harness.get("/xxx/..%2ffile1.html")
            .and_then(|res| res.body().concat2())
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is file1");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}

#[test]
fn prevents_from_escaping_root() {
    Harness::run(vec![
        ("file1.html", "this is file1")
    ], |harness| {
        let f1 = harness.get("/../file1.html")
            .and_then(|res| res.body().concat2())
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is file1");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));

        let f2 = harness.get("/..%2ffile1.html")
            .and_then(|res| res.body().concat2())
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is file1");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));

        let f3 = harness.get("/xxx/..%2f..%2ffile1.html")
            .and_then(|res| res.body().concat2())
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is file1");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));

        let f1: EmptyFuture = Box::new(f1);
        let f2: EmptyFuture = Box::new(f2);
        let f3: EmptyFuture = Box::new(f3);
        Box::new(future::join_all(vec![f1, f2, f3]).map(|_| ()))
    });
}

#[test]
fn sends_headers() {
    Harness::run(vec![
        ("file1.html", "this is file1")
    ], |harness| {
        let f = harness.get("/file1.html")
            .and_then(|res| {
                assert_eq!(res.status(), StatusCode::Ok);
                assert_eq!(res.headers().get(), Some(&header::ContentLength(13)));
                assert!(res.headers().get::<header::LastModified>().is_some());
                assert!(res.headers().get::<header::ETag>().is_some());
                assert_eq!(res.headers().get(), Some(&header::CacheControl(vec![
                    header::CacheDirective::Public,
                    header::CacheDirective::MaxAge(3600)
                ])));
                res.body().concat2()
            })
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is file1");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}

#[test]
fn serves_file_with_old_if_modified_since() {
    Harness::run(vec![
        ("file1.html", "this is file1")
    ], |harness| {
        let mut req = Request::new(Method::Get, "/file1.html".parse().unwrap());
        req.headers_mut().set(header::IfModifiedSince(header::HttpDate::from(
            SystemTime::now() - Duration::from_secs(3600)
        )));
        let f = harness.request(req)
            .and_then(|res| res.body().concat2())
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is file1");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}

#[test]
fn serves_file_with_new_if_modified_since() {
    Harness::run(vec![
        ("file1.html", "this is file1")
    ], |harness| {
        let mut req = Request::new(Method::Get, "/file1.html".parse().unwrap());
        req.headers_mut().set(header::IfModifiedSince(header::HttpDate::from(
            SystemTime::now() + Duration::from_secs(3600)
        )));
        let f = harness.request(req)
            .and_then(|res| {
                assert_eq!(res.status(), StatusCode::NotModified);
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}
