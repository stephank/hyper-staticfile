extern crate chrono;
extern crate futures;
extern crate http;
extern crate hyper;
extern crate hyper_staticfile;
extern crate tempdir;
extern crate tokio;

use std::{fs, str};
use std::io::Write;
use chrono::{Duration, Utc};
use futures::{Future, Stream, future};
use http::{StatusCode, header};
use http::request::Builder as RequestBuilder;
use hyper::Body;
use hyper::service::Service;
use hyper_staticfile::Static;
use tempdir::TempDir;

type Request = http::Request<Body>;
type Response = http::Response<Body>;
type ResponseFuture = Box<Future<Item=Response, Error=String> + Send>;
type EmptyFuture = Box<Future<Item=(), Error=()> + Send + 'static>;

struct Harness {
    static_: Static,
}
impl Harness {
    fn run<F>(files: Vec<(&str, &str)>, f: F)
            where F: FnOnce(Harness) -> EmptyFuture + Send + 'static {
        let dir = TempDir::new("hyper-staticfile-tests").unwrap();
        for (subpath, contents) in files {
            let fullpath = dir.path().join(subpath);
            fs::create_dir_all(fullpath.parent().unwrap())
                .and_then(|_| fs::File::create(fullpath))
                .and_then(|mut file| file.write(contents.as_bytes()))
                .expect("failed to write fixtures");
        }

        let static_ = Static::new(dir.path().clone())
            .with_cache_headers(3600);

        tokio::run(future::lazy(move || {
            f(Harness { static_ })
        }));
    }

    fn request(&mut self, req: Request) -> ResponseFuture {
        self.static_.call(req)
    }

    fn get(&mut self, path: &str) -> ResponseFuture {
        let req = RequestBuilder::new()
            .uri(path)
            .body(Body::empty())
            .unwrap();
        self.request(req)
    }
}

#[test]
fn serves_non_default_file_from_absolute_root_path() {
    Harness::run(vec![
        ("file1.html", "this is file1")
    ], |mut harness| {
        let f = harness.get("/file1.html")
            .and_then(|res| res.into_body().concat2().map_err(|e| e.to_string()))
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
    ], |mut harness| {
        let f = harness.get("/index.html")
            .and_then(|res| res.into_body().concat2().map_err(|e| e.to_string()))
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
    Harness::run(vec![], |mut harness| {
        let f = harness.get("/")
            .and_then(|res|  {
                assert_eq!(res.status(), StatusCode::NOT_FOUND);
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
    ], |mut harness| {
        let f = harness.get("/dir")
            .and_then(|res|  {
                assert_eq!(res.status(), StatusCode::MOVED_PERMANENTLY);

                let url = res.headers().get(header::LOCATION).unwrap();
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
    ], |mut harness| {
        let f = harness.get("/has%20space.html")
            .and_then(|res| {
                res.into_body().concat2().map_err(|e| e.to_string())
            })
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
    ], |mut harness| {
        let f = harness.get("/xxx/../index.html")
            .and_then(|res| res.into_body().concat2().map_err(|e| e.to_string()))
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
    ], |mut harness| {
        let f = harness.get("/xxx/..%2ffile1.html")
            .and_then(|res| res.into_body().concat2().map_err(|e| e.to_string()))
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
    ], |mut harness| {
        let f1 = harness.get("/../file1.html")
            .and_then(|res| res.into_body().concat2().map_err(|e| e.to_string()))
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is file1");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));

        let f2 = harness.get("/..%2ffile1.html")
            .and_then(|res| res.into_body().concat2().map_err(|e| e.to_string()))
            .and_then(|body| {
                assert_eq!(str::from_utf8(&body).unwrap(), "this is file1");
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));

        let f3 = harness.get("/xxx/..%2f..%2ffile1.html")
            .and_then(|res| res.into_body().concat2().map_err(|e| e.to_string()))
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
    ], |mut harness| {
        let f = harness.get("/file1.html")
            .and_then(|res| {
                assert_eq!(res.status(), StatusCode::OK);
                assert_eq!(res.headers().get(header::CONTENT_LENGTH).unwrap(), "13");
                assert!(res.headers().get(header::LAST_MODIFIED).is_some());
                assert!(res.headers().get(header::ETAG).is_some());
                assert_eq!(res.headers().get(header::CACHE_CONTROL).unwrap(), "public, max-age=3600");
                res.into_body().concat2().map_err(|e| e.to_string())
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
    ], |mut harness| {
        let if_modified = Utc::now() - Duration::seconds(3600);
        let req = RequestBuilder::new()
            .uri("/file1.html")
            .header(header::IF_MODIFIED_SINCE, if_modified.to_rfc2822().as_str())
            .body(Body::empty())
            .unwrap();
        let f = harness.request(req)
            .and_then(|res| res.into_body().concat2().map_err(|e| e.to_string()))
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
    ], |mut harness| {
        let if_modified = Utc::now() + Duration::seconds(3600);
        let req = RequestBuilder::new()
            .uri("/file1.html")
            .header(header::IF_MODIFIED_SINCE, if_modified.to_rfc2822().as_str())
            .body(Body::empty())
            .unwrap();
        let f = harness.request(req)
            .and_then(|res| {
                assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
                future::ok(())
            })
            .map_err(|err| panic!("{}", err));
        Box::new(f)
    });
}
