use chrono::{Duration, Utc};
use futures_util::stream::StreamExt;
use http::{header, Request, StatusCode};
use hyper_staticfile::Static;
use std::future::Future;
use std::io::{Error as IoError, Write};
use std::{fs, str};
use tempdir::TempDir;

type Response = hyper::Response<hyper::Body>;
type ResponseResult = Result<Response, IoError>;

struct Harness {
    #[allow(unused)]
    dir: TempDir,
    static_: Static,
}
impl Harness {
    fn new(files: Vec<(&str, &str)>) -> Harness {
        let dir = TempDir::new("hyper-staticfile-tests").unwrap();
        for (subpath, contents) in files {
            let fullpath = dir.path().join(subpath);
            fs::create_dir_all(fullpath.parent().unwrap())
                .and_then(|_| fs::File::create(fullpath))
                .and_then(|mut file| file.write(contents.as_bytes()))
                .expect("failed to write fixtures");
        }

        let mut static_ = Static::new(dir.path().clone());
        static_.cache_headers(Some(3600));

        Harness { dir, static_ }
    }

    fn request<B>(&self, req: Request<B>) -> impl Future<Output = ResponseResult> {
        self.static_.clone().serve(req)
    }

    fn get(&self, path: &str) -> impl Future<Output = ResponseResult> {
        let req = Request::builder()
            .uri(path)
            .body(())
            .expect("unable to build request");
        self.request(req)
    }
}

async fn read_body(req: Response) -> String {
    let mut buf = vec![];
    let mut body = req.into_body();
    loop {
        match body.next().await {
            None => break,
            Some(Err(err)) => panic!("{:?}", err),
            Some(Ok(chunk)) => buf.extend_from_slice(&chunk[..]),
        }
    }
    String::from_utf8(buf).unwrap()
}

#[tokio::test]
async fn serves_non_default_file_from_absolute_root_path() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let res = harness.get("/file1.html").await.unwrap();
    assert_eq!(read_body(res).await, "this is file1");
}

#[tokio::test]
async fn serves_default_file_from_absolute_root_path() {
    let harness = Harness::new(vec![("index.html", "this is index")]);

    let res = harness.get("/index.html").await.unwrap();
    assert_eq!(read_body(res).await, "this is index");
}

#[tokio::test]
async fn serves_default_file_from_empty_root_path() {
    let harness = Harness::new(vec![("index.html", "this is index")]);

    let res = harness.get("/").await.unwrap();
    assert_eq!(read_body(res).await, "this is index");
}

#[tokio::test]
async fn returns_404_if_file_not_found() {
    let harness = Harness::new(vec![]);

    let res = harness.get("/").await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn redirects_if_trailing_slash_is_missing() {
    let harness = Harness::new(vec![("dir/index.html", "this is index")]);

    let res = harness.get("/dir").await.unwrap();
    assert_eq!(res.status(), StatusCode::MOVED_PERMANENTLY);

    let url = res.headers().get(header::LOCATION).unwrap();
    assert_eq!(url, "/dir/");
}

#[tokio::test]
async fn decodes_percent_notation() {
    let harness = Harness::new(vec![("has space.html", "file with funky chars")]);

    let res = harness.get("/has%20space.html").await.unwrap();
    assert_eq!(read_body(res).await, "file with funky chars");
}

#[tokio::test]
async fn normalizes_path() {
    let harness = Harness::new(vec![("index.html", "this is index")]);

    let res = harness.get("/xxx/../index.html").await.unwrap();
    assert_eq!(read_body(res).await, "this is index");
}

#[tokio::test]
async fn normalizes_percent_encoded_path() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let res = harness.get("/xxx/..%2ffile1.html").await.unwrap();
    assert_eq!(read_body(res).await, "this is file1");
}

#[tokio::test]
async fn prevents_from_escaping_root() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let res = harness.get("/../file1.html").await.unwrap();
    assert_eq!(read_body(res).await, "this is file1");

    let res = harness.get("/..%2ffile1.html").await.unwrap();
    assert_eq!(read_body(res).await, "this is file1");

    let res = harness.get("/xxx/..%2f..%2ffile1.html").await.unwrap();
    assert_eq!(read_body(res).await, "this is file1");
}

#[tokio::test]
async fn sends_headers() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let res = harness.get("/file1.html").await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers().get(header::CONTENT_LENGTH).unwrap(), "13");
    assert!(res.headers().get(header::LAST_MODIFIED).is_some());
    assert!(res.headers().get(header::ETAG).is_some());
    assert_eq!(
        res.headers().get(header::CACHE_CONTROL).unwrap(),
        "public, max-age=3600"
    );
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static("text/html"))
    );

    assert_eq!(read_body(res).await, "this is file1");
}

#[tokio::test]
async fn changes_content_type_on_extension() {
    let harness = Harness::new(vec![("file1.gif", "this is file1")]);

    let res = harness.get("/file1.gif").await.unwrap();
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static("image/gif"))
    );
}

#[tokio::test]
async fn serves_file_with_old_if_modified_since() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let if_modified = Utc::now() - Duration::seconds(3600);
    let req = Request::builder()
        .uri("/file1.html")
        .header(header::IF_MODIFIED_SINCE, if_modified.to_rfc2822().as_str())
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    assert_eq!(read_body(res).await, "this is file1");
}

#[tokio::test]
async fn serves_file_with_new_if_modified_since() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let if_modified = Utc::now() + Duration::seconds(3600);
    let req = Request::builder()
        .uri("/file1.html")
        .header(header::IF_MODIFIED_SINCE, if_modified.to_rfc2822().as_str())
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
}
