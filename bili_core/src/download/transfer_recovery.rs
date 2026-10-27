use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn locator(url: Url, expected_size: Option<u64>) -> ResolvedLocator {
    ResolvedLocator {
        url,
        referer: "https://www.bilibili.com/".into(),
        identity: "test representation".into(),
        expected_size,
        duration_ms: None,
        allowed_origins: Vec::new(),
    }
}

async fn serve(responses: Vec<String>) -> (Url, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                if socket.read_exact(&mut byte).await.is_err() {
                    break;
                }
                request.push(byte[0]);
            }
            requests.push(String::from_utf8_lossy(&request).into_owned());
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        }
        requests
    });
    (
        Url::parse(&format!("http://{address}/media")).unwrap(),
        task,
    )
}
async fn serve_until_stopped(
    responses: Vec<String>,
) -> (
    Url,
    tokio::task::JoinHandle<Vec<String>>,
    tokio::sync::oneshot::Sender<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut responses = responses.into_iter();
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                accepted = listener.accept() => {
                    let (mut socket, _) = accepted.unwrap();
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while !request.ends_with(b"\r\n\r\n") {
                        if socket.read_exact(&mut byte).await.is_err() { break; }
                        request.push(byte[0]);
                    }
                    requests.push(String::from_utf8_lossy(&request).into_owned());
                    let Some(response) = responses.next() else { break; };
                    socket.write_all(response.as_bytes()).await.unwrap();
                    socket.shutdown().await.unwrap();
                }
            }
        }
        requests
    });
    (
        Url::parse(&format!("http://{address}/media")).unwrap(),
        task,
        stop,
    )
}

fn response(status: &str, headers: &str, body: &str) -> String {
    format!("HTTP/1.1 {status}\r\n{headers}Connection: close\r\n\r\n{body}")
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

async fn run(url: Url, path: &Path, checkpoint: &mut ResourceCheckpoint) -> Result<()> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    transfer(
        &client,
        &locator(url, checkpoint.expected_total),
        path,
        checkpoint,
        TransferOptions {
            cancel: &CancellationToken::new(),
            rate_limit: None,
        },
        &RetryPolicy {
            attempts: 1,
            base_delay: Duration::ZERO,
        },
        |_| {},
    )
    .await
}

#[tokio::test]
async fn streams_identity_encoded_full_resource_and_persists_digest() {
    let body = "payload";
    let (url, server) = serve(vec![response(
        "200 OK",
        "Content-Length: 7\r\nETag: \"v1\"\r\n",
        body,
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    let mut checkpoint = ResourceCheckpoint::default();

    run(url, &path, &mut checkpoint).await.unwrap();

    assert_eq!(tokio::fs::read(&path).await.unwrap(), body.as_bytes());
    assert_eq!(checkpoint.durable_bytes, body.len() as u64);
    assert_eq!(checkpoint.expected_total, Some(body.len() as u64));
    assert_eq!(
        checkpoint.sha256.as_deref(),
        Some(digest(body.as_bytes()).as_str())
    );
    assert!(checkpoint.complete);
    let request = server.await.unwrap().remove(0);
    assert!(request
        .to_ascii_lowercase()
        .contains("accept-encoding: identity"));
    assert!(request
        .to_ascii_lowercase()
        .contains("referer: https://www.bilibili.com/"));
    assert!(!request.to_ascii_lowercase().contains("cookie:"));
}

#[tokio::test]
async fn resumes_only_the_validated_content_range() {
    let (url, server) = serve(vec![response(
        "206 Partial Content",
        "Content-Length: 3\r\nContent-Range: bytes 3-5/6\r\nETag: \"v1\"\r\n",
        "def",
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    tokio::fs::write(&path, b"abc").await.unwrap();
    let mut checkpoint = ResourceCheckpoint {
        durable_bytes: 3,
        expected_total: Some(6),
        strong_etag: Some("\"v1\"".into()),
        sha256: Some(digest(b"abc")),
        complete: false,
    };

    run(url, &path, &mut checkpoint).await.unwrap();

    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"abcdef");
    assert!(checkpoint.complete);
    let request = server.await.unwrap().remove(0).to_ascii_lowercase();
    assert!(request.contains("range: bytes=3-"));
    assert!(request.contains("if-range: \"v1\""));
}

#[tokio::test]
async fn resume_without_checkpoint_hash_refetches_the_local_prefix() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            if socket.read_exact(&mut byte).await.is_err() {
                break;
            }
            request.push(byte[0]);
        }
        let request = String::from_utf8_lossy(&request).into_owned();
        let reply = if request.to_ascii_lowercase().contains("range: bytes=3-") {
            response(
                "206 Partial Content",
                "Content-Length: 3\r\nContent-Range: bytes 3-5/6\r\nETag: \"v1\"\r\n",
                "def",
            )
        } else {
            response("200 OK", "Content-Length: 6\r\nETag: \"v1\"\r\n", "abcdef")
        };
        socket.write_all(reply.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
        request
    });
    let url = Url::parse(&format!("http://{address}/media")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    tokio::fs::write(&path, b"bad").await.unwrap();
    let mut checkpoint = ResourceCheckpoint {
        durable_bytes: 3,
        expected_total: Some(6),
        strong_etag: Some("\"v1\"".into()),
        sha256: None,
        complete: false,
    };

    let result = run(url, &path, &mut checkpoint).await;
    let request = server.await.unwrap().to_ascii_lowercase();

    assert!(result.is_ok());
    let range = request
        .lines()
        .find_map(|line| line.strip_prefix("range: "));
    assert!(range.is_none_or(|value| value == "bytes=0-"));
    assert_eq!(tokio::fs::read(path).await.unwrap(), b"abcdef");
}

#[tokio::test]
async fn checkpoint_resource_shorter_file_discards_unproven_offset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    let sidecar = checkpoint_path(&path);
    tokio::fs::write(&path, b"ab").await.unwrap();
    let mut checkpoint = ResourceCheckpoint {
        durable_bytes: 3,
        expected_total: Some(6),
        strong_etag: Some("\"v1\"".into()),
        sha256: Some(digest(b"abc")),
        complete: false,
    };

    checkpoint_resource(&path, &sidecar, &mut checkpoint)
        .await
        .unwrap();
    let saved = load_checkpoint(&path).unwrap();

    assert_eq!(checkpoint.durable_bytes, 0);
    assert!(checkpoint.sha256.is_none());
    assert!(checkpoint.strong_etag.is_none());
    assert_eq!(saved.durable_bytes, 0);
    assert!(saved.sha256.is_none());
    assert!(saved.strong_etag.is_none());
    assert_eq!(tokio::fs::read(path).await.unwrap(), b"");
}

#[tokio::test]
async fn ignored_range_restarts_instead_of_appending() {
    let (url, server) = serve(vec![response(
        "200 OK",
        "Content-Length: 6\r\nETag: \"v2\"\r\n",
        "uvwxyz",
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    tokio::fs::write(&path, b"abc").await.unwrap();
    let mut checkpoint = ResourceCheckpoint {
        durable_bytes: 3,
        expected_total: Some(6),
        strong_etag: Some("\"v1\"".into()),
        sha256: Some(digest(b"abc")),
        complete: false,
    };

    run(url, &path, &mut checkpoint).await.unwrap();

    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"uvwxyz");
    assert_eq!(checkpoint.strong_etag.as_deref(), Some("\"v2\""));
    let request = server.await.unwrap().remove(0).to_ascii_lowercase();
    assert!(request.contains("range: bytes=3-"));
}

#[tokio::test]
async fn rejects_wrong_content_range_before_writing() {
    let (url, server) = serve(vec![response(
        "206 Partial Content",
        "Content-Length: 2\r\nContent-Range: bytes 1-2/3\r\nETag: \"v1\"\r\n",
        "bc",
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    let mut checkpoint = ResourceCheckpoint::default();

    let result = run(url, &path, &mut checkpoint).await;

    assert_eq!(result.unwrap_err().code, ErrorCode::Transport);
    assert!(!path.exists());
    server.await.unwrap();
}

#[tokio::test]
async fn accepts_416_only_for_matching_total_validator_and_hash() {
    let (url, server) = serve(vec![response(
        "416 Range Not Satisfiable",
        "Content-Range: bytes */3\r\nETag: \"v1\"\r\n",
        "",
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    tokio::fs::write(&path, b"abc").await.unwrap();
    let mut checkpoint = ResourceCheckpoint {
        durable_bytes: 3,
        expected_total: Some(3),
        strong_etag: Some("\"v1\"".into()),
        sha256: Some(digest(b"abc")),
        complete: false,
    };

    run(url, &path, &mut checkpoint).await.unwrap();

    assert!(checkpoint.complete);
    assert_eq!(checkpoint.sha256.as_deref(), Some(digest(b"abc").as_str()));
    server.await.unwrap();
}

#[tokio::test]
async fn refuses_416_when_validator_does_not_match() {
    let (url, server) = serve(vec![response(
        "416 Range Not Satisfiable",
        "Content-Range: bytes */3\r\nETag: \"v2\"\r\n",
        "",
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    tokio::fs::write(&path, b"abc").await.unwrap();
    let mut checkpoint = ResourceCheckpoint {
        durable_bytes: 3,
        expected_total: Some(3),
        strong_etag: Some("\"v1\"".into()),
        sha256: Some(digest(b"abc")),
        complete: false,
    };

    let error = run(url, &path, &mut checkpoint).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::RepresentationChanged);
    assert!(!checkpoint.complete);
    server.await.unwrap();
}

#[tokio::test]
async fn follows_only_allowed_redirects_without_forwarding_credentials() {
    let (url, server) = serve(vec![
        response("302 Found", "Location: /redirected\r\n", ""),
        response("200 OK", "Content-Length: 2\r\n", "ok"),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    let mut checkpoint = ResourceCheckpoint::default();

    run(url, &path, &mut checkpoint).await.unwrap();

    assert_eq!(tokio::fs::read(path).await.unwrap(), b"ok");
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|request| !request.to_ascii_lowercase().contains("cookie:")));
    assert!(requests
        .iter()
        .all(|request| !request.to_ascii_lowercase().contains("authorization:")));
}

#[tokio::test]
async fn refuses_redirects_to_unlisted_hosts() {
    let (url, server) = serve(vec![response(
        "302 Found",
        "Location: https://unlisted.invalid/media\r\n",
        "",
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    let mut checkpoint = ResourceCheckpoint::default();

    let error = run(url, &path, &mut checkpoint).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::Transport);
    server.await.unwrap();
}

#[tokio::test]
async fn refreshable_expiry_is_reported_to_service() {
    let (url, server) = serve(vec![response("403 Forbidden", "Content-Length: 0\r\n", "")]).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    let mut checkpoint = ResourceCheckpoint::default();

    let error = run(url, &path, &mut checkpoint).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::Transport);
    assert!(error.retryable);
    server.await.unwrap();
}

#[tokio::test]
async fn weak_validator_discards_partial_bytes_before_request() {
    let (url, server) = serve(vec![response(
        "200 OK",
        "Content-Length: 6\r\nETag: \"v2\"\r\n",
        "uvwxyz",
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    tokio::fs::write(&path, b"abc").await.unwrap();
    let mut checkpoint = ResourceCheckpoint {
        durable_bytes: 3,
        expected_total: Some(6),
        strong_etag: Some("W/\"v1\"".into()),
        sha256: Some(digest(b"abc")),
        complete: false,
    };

    run(url, &path, &mut checkpoint).await.unwrap();

    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"uvwxyz");
    let request = server.await.unwrap().remove(0).to_ascii_lowercase();
    assert!(!request.contains("range:"));
}

#[tokio::test]
async fn short_body_never_advances_durable_checkpoint() {
    let (url, server) = serve(vec![response(
        "200 OK",
        "Content-Length: 6\r\nETag: \"v1\"\r\n",
        "abc",
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    let mut checkpoint = ResourceCheckpoint::default();

    let error = run(url, &path, &mut checkpoint).await.unwrap_err();

    assert_eq!(error.code, ErrorCode::RetryExhausted);
    assert_eq!(checkpoint.durable_bytes, 0);
    assert!(!checkpoint.complete);
    assert_eq!(tokio::fs::metadata(path).await.unwrap().len(), 0);
    server.await.unwrap();
}

#[test]
fn honors_delta_seconds_and_http_date_retry_after() {
    let mut headers = header::HeaderMap::new();
    headers.insert(header::RETRY_AFTER, header::HeaderValue::from_static("7"));
    assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(7)));

    let target = std::time::SystemTime::now() + Duration::from_secs(20);
    let date = httpdate::fmt_http_date(target);
    headers.insert(
        header::RETRY_AFTER,
        header::HeaderValue::from_bytes(date.as_bytes()).unwrap(),
    );
    let delay = parse_retry_after(&headers).unwrap();
    assert!(delay <= Duration::from_secs(20));
    assert!(delay >= Duration::from_secs(18));
}

#[tokio::test]
async fn cancellation_during_rate_limit_flushes_latest_bytes() {
    let (url, server) = serve(vec![response(
        "200 OK",
        "Content-Length: 3\r\nETag: \"v1\"\r\n",
        "xyz",
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let cancel = CancellationToken::new();
    let progress_cancel = cancel.clone();
    let mut checkpoint = ResourceCheckpoint::default();

    let result = transfer(
        &client,
        &locator(url, None),
        &path,
        &mut checkpoint,
        TransferOptions {
            cancel: &cancel,
            rate_limit: Some(1),
        },
        &RetryPolicy {
            attempts: 1,
            base_delay: Duration::ZERO,
        },
        move |_| progress_cancel.cancel(),
    )
    .await;

    assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
    assert!(checkpoint.durable_bytes > 0);
    assert_eq!(
        tokio::fs::metadata(&path).await.unwrap().len(),
        checkpoint.durable_bytes
    );
    let persisted = load_checkpoint(&path).unwrap();
    assert_eq!(persisted.durable_bytes, checkpoint.durable_bytes);
    assert_eq!(persisted.sha256, checkpoint.sha256);
    assert!(!persisted.complete);
    server.await.unwrap();
}

#[tokio::test]
async fn retries_transient_responses_with_a_finite_attempt_budget() {
    let (url, server) = serve(vec![
        response(
            "503 Service Unavailable",
            "Content-Length: 0\r\nRetry-After: 0\r\n",
            "",
        ),
        response("200 OK", "Content-Length: 2\r\nETag: \"v1\"\r\n", "ok"),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mut checkpoint = ResourceCheckpoint::default();

    transfer(
        &client,
        &locator(url, None),
        &path,
        &mut checkpoint,
        TransferOptions {
            cancel: &CancellationToken::new(),
            rate_limit: None,
        },
        &RetryPolicy {
            attempts: 2,
            base_delay: Duration::ZERO,
        },
        |_| {},
    )
    .await
    .unwrap();

    assert_eq!(tokio::fs::read(path).await.unwrap(), b"ok");
    assert_eq!(server.await.unwrap().len(), 2);
}

#[tokio::test]
async fn stops_after_retry_policy_attempts_are_exhausted() {
    let (url, server, stop) = serve_until_stopped(vec![
        response(
            "503 Service Unavailable",
            "Content-Length: 0\r\nRetry-After: 0\r\n",
            "",
        ),
        response(
            "503 Service Unavailable",
            "Content-Length: 0\r\nRetry-After: 0\r\n",
            "",
        ),
        response("200 OK", "Content-Length: 2\r\n", "ok"),
    ])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("resource.bin");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mut checkpoint = ResourceCheckpoint::default();

    let result = transfer(
        &client,
        &locator(url, None),
        &path,
        &mut checkpoint,
        TransferOptions {
            cancel: &CancellationToken::new(),
            rate_limit: None,
        },
        &RetryPolicy {
            attempts: 2,
            base_delay: Duration::ZERO,
        },
        |_| {},
    )
    .await;
    let _ = stop.send(());
    let requests = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("retry fixture shutdown timed out")
        .unwrap();

    assert_eq!(result.unwrap_err().code, ErrorCode::RetryExhausted);
    assert_eq!(requests.len(), 2);
}
