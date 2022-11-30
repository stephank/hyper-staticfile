use futures_util::stream::StreamExt;
use http::{header, Request, StatusCode};
use httpdate::fmt_http_date;
use hyper_staticfile::Static;
use std::future::Future;
use std::io::{Cursor, Error as IoError, Write};
use std::process::Command;
use std::time::{Duration, SystemTime};
use std::{fs, str};
use tempdir::TempDir;

type Response = hyper::Response<hyper::Body>;
type ResponseResult = Result<Response, IoError>;

struct Harness {
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
                .and_then(|mut file| file.write_all(contents.as_bytes()))
                .expect("failed to write fixtures");
        }

        let mut static_ = Static::new(dir.path().clone());
        static_.cache_headers(Some(3600));

        Harness { dir, static_ }
    }

    fn append(&self, subpath: &str, content: &str) {
        let path = self.dir.path().join(subpath);
        let mut f = fs::File::options()
            .append(true)
            .open(path)
            .expect("failed to append to fixture");
        f.write_all(content.as_bytes())
            .expect("failed to append to fixture");
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
async fn content_length() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let res = harness.get("/file1.html").await.unwrap();
    harness.append("file1.html", "more content");
    assert_eq!(res.headers().get(header::CONTENT_LENGTH).unwrap(), "13");
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

    let if_modified = SystemTime::now() - Duration::from_secs(3600);
    let req = Request::builder()
        .uri("/file1.html")
        .header(header::IF_MODIFIED_SINCE, fmt_http_date(if_modified))
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    assert_eq!(read_body(res).await, "this is file1");
}

#[tokio::test]
async fn serves_file_with_new_if_modified_since() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let if_modified = SystemTime::now() + Duration::from_secs(3600);
    let req = Request::builder()
        .uri("/file1.html")
        .header(header::IF_MODIFIED_SINCE, fmt_http_date(if_modified))
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
}

#[cfg(target_family = "unix")]
#[tokio::test]
async fn last_modified_is_gmt() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let mut file_path = harness.dir.path().to_path_buf();
    file_path.push("file1.html");
    let status = Command::new("touch")
        .args(&["-t", "198510260122.00"])
        .arg(file_path)
        .env("TZ", "UTC")
        .status()
        .unwrap();
    assert!(status.success());

    let res = harness.get("/file1.html").await.unwrap();
    assert_eq!(
        res.headers()
            .get(header::LAST_MODIFIED)
            .map(|val| val.to_str().unwrap()),
        Some("Sat, 26 Oct 1985 01:22:00 GMT")
    );
}

#[cfg(target_family = "unix")]
#[tokio::test]
async fn no_headers_for_invalid_mtime() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let mut file_path = harness.dir.path().to_path_buf();
    file_path.push("file1.html");
    let status = Command::new("touch")
        .args(&["-t", "197001010000.01"])
        .arg(file_path)
        .env("TZ", "UTC")
        .status()
        .unwrap();
    assert!(status.success());

    let res = harness.get("/file1.html").await.unwrap();
    assert!(res.headers().get(header::ETAG).is_none());
}

#[tokio::test]
async fn serves_file_ranges_beginning() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let req = Request::builder()
        .uri("/file1.html")
        .header(header::RANGE, "bytes=0-3")
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    assert_eq!(read_body(res).await, "this");
}

#[tokio::test]
async fn serves_file_ranges_end() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let req = Request::builder()
        .uri("/file1.html")
        .header(header::RANGE, "bytes=5-")
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    assert_eq!(read_body(res).await, "is file1");
}

#[tokio::test]
async fn serves_file_ranges_multi() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let req = Request::builder()
        .uri("/file1.html")
        .header(header::RANGE, "bytes=0-3, 5-")
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.starts_with("multipart/byteranges; boundary="));
    let boundary = &content_type[31..];

    let mut body_expectation = Cursor::new(Vec::new());
    write!(&mut body_expectation, "--{}\r\n", boundary).unwrap();
    write!(&mut body_expectation, "Content-Range: bytes 0-3/13\r\n").unwrap();
    write!(&mut body_expectation, "Content-Type: text/html\r\n").unwrap();
    write!(&mut body_expectation, "\r\n").unwrap();
    write!(&mut body_expectation, "this\r\n").unwrap();

    write!(&mut body_expectation, "--{}\r\n", boundary).unwrap();
    write!(&mut body_expectation, "Content-Range: bytes 5-12/13\r\n").unwrap();
    write!(&mut body_expectation, "Content-Type: text/html\r\n").unwrap();
    write!(&mut body_expectation, "\r\n").unwrap();
    write!(&mut body_expectation, "is file1\r\n").unwrap();
    write!(&mut body_expectation, "--{}--\r\n", boundary).unwrap();
    let body_expectation = String::from_utf8(body_expectation.into_inner()).unwrap();
    assert_eq!(read_body(res).await, body_expectation);
}

#[tokio::test]
async fn serves_file_ranges_multi_assert_content_length_correct() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let req = Request::builder()
        .uri("/file1.html")
        .header(header::RANGE, "bytes=0-3, 5-")
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();

    let content_length: usize = res
        .headers()
        .get(header::CONTENT_LENGTH)
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();

    assert_eq!(read_body(res).await.len(), content_length);
}

#[tokio::test]
async fn serves_file_ranges_if_range_negative() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let req = Request::builder()
        .uri("/file1.html")
        .header(header::RANGE, "bytes=5-")
        .header(header::IF_RANGE, "Sat, 26 Oct 1985 01:22:00 GMT")
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    // whole thing comes back since If-Range didn't match
    assert_eq!(read_body(res).await, "this is file1");
}

#[tokio::test]
async fn serves_file_ranges_if_range_etag_positive() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    // first request goes out without etag to fetch etag
    let req = Request::builder()
        .uri("/file1.html")
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    let etag_value = res.headers().get(header::ETAG).unwrap();

    let req = Request::builder()
        .uri("/file1.html")
        .header(header::RANGE, "bytes=5-")
        .header(header::IF_RANGE, etag_value)
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    assert_eq!(read_body(res).await, "is file1");
}

#[tokio::test]
async fn serves_requested_range_not_satisfiable_when_at_end() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);
    let req = Request::builder()
        .uri("/file1.html")
        .header(header::RANGE, "bytes=13-")
        .body(())
        .expect("unable to build request");

    let res = harness.request(req).await.unwrap();
    assert_eq!(res.status(), hyper::StatusCode::RANGE_NOT_SATISFIABLE);
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn ignore_windows_drive_letter() {
    let harness = Harness::new(vec![("file1.html", "this is file1")]);

    let res = harness.get("/c:/file1.html").await.unwrap();
    assert_eq!(read_body(res).await, "this is file1");
}
