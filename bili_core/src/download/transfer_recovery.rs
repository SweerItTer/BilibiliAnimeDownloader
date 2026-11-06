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

#[tokio::test]
async fn concurrent_resources_share_one_batch_budget() {
    let root = tempfile::tempdir().unwrap();
    let payload = "x".repeat(100);
    let headers = "Content-Length: 100\r\nETag: \"shared-budget\"\r\n";
    let (first_url, first_server) = serve(vec![response("200 OK", headers, &payload)]).await;
    let (second_url, second_server) = serve(vec![response("200 OK", headers, &payload)]).await;
    let budget = RateBudget::new(1000);
    let client = reqwest::Client::new();
    let cancel = CancellationToken::new();
    let policy = RetryPolicy {
        attempts: 1,
        base_delay: Duration::ZERO,
    };
    let mut first = ResourceCheckpoint::default();
    let mut second = ResourceCheckpoint::default();
    let first_path = root.path().join("first.media");
    let second_path = root.path().join("second.media");
    let first_locator = locator(first_url, Some(100));
    let second_locator = locator(second_url, Some(100));
    let started = Instant::now();
    let (a, b) = tokio::join!(
        transfer(
            &client,
            &first_locator,
            &first_path,
            &mut first,
            TransferOptions {
                cancel: &cancel,
                rate_limit: None,
                budget: Some(&budget)
            },
            &policy,
            |_| {}
        ),
        transfer(
            &client,
            &second_locator,
            &second_path,
            &mut second,
            TransferOptions {
                cancel: &cancel,
                rate_limit: None,
                budget: Some(&budget)
            },
            &policy,
            |_| {}
        )
    );
    first_server.await.unwrap();
    second_server.await.unwrap();
    a.unwrap();
    b.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(190));
    assert_eq!(std::fs::read(first_path).unwrap(), payload.as_bytes());
    assert_eq!(std::fs::read(second_path).unwrap(), payload.as_bytes());
    assert_eq!(first.durable_bytes + second.durable_bytes, 200);
}

#[tokio::test]
async fn cancelled_batch_debt_blocks_resumed_resource_before_receiving_more_bytes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("paused.media");
    let payload = "x".repeat(1024);
    let (url, server) = serve(vec![response(
        "200 OK",
        "Content-Length: 1024\r\nETag: \"pause-debt\"\r\n",
        &payload[..512],
    )])
    .await;
    let budget = RateBudget::new(1);
    let client = reqwest::Client::new();
    let policy = RetryPolicy {
        attempts: 1,
        base_delay: Duration::ZERO,
    };
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let mut checkpoint = ResourceCheckpoint::default();
    let first = transfer(
        &client,
        &locator(url, Some(1024)),
        &path,
        &mut checkpoint,
        TransferOptions {
            cancel: &cancel,
            rate_limit: None,
            budget: Some(&budget),
        },
        &policy,
        move |bytes| {
            if bytes > 0 {
                signal.cancel();
            }
        },
    )
    .await;
    server.await.unwrap();
    assert_eq!(first.unwrap_err().code, ErrorCode::Cancelled);
    let retained = checkpoint.durable_bytes;
    assert!(retained > 0 && retained < 1024);

    let headers = format!("Content-Length: {}\r\nContent-Range: bytes {retained}-1023/1024\r\nETag: \"pause-debt\"\r\n", 1024 - retained);
    let (url, server) = serve(vec![response(
        "206 Partial Content",
        &headers,
        &payload[retained as usize..],
    )])
    .await;
    let retry_cancel = CancellationToken::new();
    let retry_locator = locator(url, Some(1024));
    let (premature, result) = {
        let resumed = transfer(
            &client,
            &retry_locator,
            &path,
            &mut checkpoint,
            TransferOptions {
                cancel: &retry_cancel,
                rate_limit: None,
                budget: Some(&budget),
            },
            &policy,
            |_| {},
        );
        tokio::pin!(resumed);
        match tokio::time::timeout(Duration::from_millis(50), resumed.as_mut()).await {
            Ok(result) => (true, result),
            Err(_) => {
                retry_cancel.cancel();
                (false, resumed.await)
            }
        }
    };
    let requests = server.await.unwrap();
    assert!(!premature, "resume must retain the previous batch debt");
    assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
    assert!(requests[0].contains(&format!("range: bytes={retained}-")));
    assert_eq!(
        checkpoint.durable_bytes, retained,
        "resume must wait before consuming another response chunk"
    );
    assert_eq!(std::fs::metadata(&path).unwrap().len(), retained);
    assert_eq!(load_checkpoint(&path).unwrap().durable_bytes, retained);
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
            budget: None,
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
    for shared in [false, true] {
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
        let budget = RateBudget::new(1);

        let result = transfer(
            &client,
            &locator(url, None),
            &path,
            &mut checkpoint,
            TransferOptions {
                cancel: &cancel,
                rate_limit: if shared { None } else { Some(1) },
                budget: shared.then_some(&budget),
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
            budget: None,
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
            budget: None,
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
