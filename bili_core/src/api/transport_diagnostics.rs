use super::*;
use serde_json::json;
use std::time::Duration;
use tokio::net::TcpListener;

#[test]
fn http_status_is_serialized_without_changing_error_classification() {
    let error = http_response_failure(403);
    assert_eq!(error.code, ErrorCode::Transport);
    assert!(!error.retryable);
    assert_eq!(error.http_status, Some(403));

    let serialized = serde_json::to_value(&error).unwrap();
    assert_eq!(serialized["code"], "Transport");
    assert_eq!(serialized["http_status"].as_u64(), Some(403));
    assert!(serialized.get("native_code").is_none());
    let without_status = CoreError::new(ErrorCode::Transport, "request Bilibili API");
    assert_eq!(without_status.http_status, None);
    assert!(serde_json::to_value(without_status)
        .unwrap()
        .get("http_status")
        .is_none());

    let legacy: CoreError = serde_json::from_value(json!({
        "code": "Transport",
        "operation": "request Bilibili API",
        "retryable": false
    }))
    .unwrap();
    assert_eq!(legacy.http_status, None);
    assert_eq!(legacy.native_code, None);
    assert!(serde_json::to_value(legacy)
        .unwrap()
        .get("http_status")
        .is_none());
}

#[tokio::test]
async fn request_send_failures_have_safe_operation_categories() {
    let secret = "synthetic-credential-canary";
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let timeout_error = client
        .get(format!("http://{address}/private?cookie={secret}"))
        .send()
        .await
        .unwrap_err();
    server.abort();
    let _ = server.await;
    assert!(timeout_error.is_timeout());
    assert_safe_transport_error(
        &request_failure(&timeout_error),
        "request Bilibili API timed out",
        true,
        secret,
    );

    let connection_error = client
        .get(format!("http://127.0.0.1:0/private?cookie={secret}"))
        .send()
        .await
        .unwrap_err();
    assert!(connection_error.is_connect());
    assert_safe_transport_error(
        &request_failure(&connection_error),
        "connect to Bilibili API",
        true,
        secret,
    );

    let generic_error = client
        .get(format!("ftp://127.0.0.1:0/private?cookie={secret}"))
        .send()
        .await
        .unwrap_err();
    assert!(!generic_error.is_timeout());
    assert!(!generic_error.is_connect());
    assert_safe_transport_error(
        &request_failure(&generic_error),
        "request Bilibili API",
        false,
        secret,
    );
}

fn assert_safe_transport_error(error: &CoreError, operation: &str, retryable: bool, secret: &str) {
    assert_eq!(error.code, ErrorCode::Transport);
    assert_eq!(error.operation, operation);
    assert_eq!(error.retryable, retryable);
    assert_eq!(error.http_status, None);
    assert!(std::error::Error::source(error).is_none());

    let rendered = [
        error.to_string(),
        format!("{error:?}"),
        serde_json::to_string(error).unwrap(),
    ];
    assert!(rendered.iter().all(|text| !text.contains(secret)));
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    extra_length: usize,
    ready: Option<tokio::sync::oneshot::Sender<()>>,
    release: Option<tokio::sync::oneshot::Receiver<()>>,
    headers: &'static str,
}

impl Reply {
    fn new(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.as_bytes().to_vec(),
            extra_length: 0,
            ready: None,
            release: None,
            headers: "",
        }
    }
}

struct HttpFixture {
    endpoint: Url,
    worker: tokio::task::JoinHandle<Vec<String>>,
}

impl HttpFixture {
    async fn start(replies: Vec<Reply>) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!(
            "http://{}/fixture?page=2&private=synthetic-canary",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let worker = tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let request = tokio::time::timeout(Duration::from_secs(5), async {
                    let mut bytes = Vec::new();
                    while !bytes.ends_with(b"\r\n\r\n") {
                        assert!(bytes.len() < 8192);
                        bytes.push(socket.read_u8().await.unwrap());
                    }
                    String::from_utf8(bytes).unwrap()
                })
                .await
                .unwrap();
                requests.push(request.lines().next().unwrap().to_owned());
                socket.write_all(format!(
                    "HTTP/1.1 {} fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{}\r\n",
                    reply.status, reply.body.len() + reply.extra_length, reply.headers,
                ).as_bytes()).await.unwrap();
                let split = if reply.release.is_some() {
                    reply.body.len() / 2
                } else {
                    reply.body.len()
                };
                socket.write_all(&reply.body[..split]).await.unwrap();
                if let Some(ready) = reply.ready {
                    let _ = ready.send(());
                }
                if let Some(release) = reply.release {
                    let _ = tokio::time::timeout(Duration::from_secs(5), release).await;
                    // A cancellation test deliberately closes the receiving body.
                    let _ = socket.write_all(&reply.body[split..]).await;
                }
            }
            requests
        });
        Self { endpoint, worker }
    }

    async fn finish(mut self) -> Vec<String> {
        match tokio::time::timeout(Duration::from_secs(6), &mut self.worker).await {
            Ok(result) => result.unwrap(),
            Err(_) => {
                self.worker.abort();
                let _ = self.worker.await;
                panic!("loopback fixture did not finish");
            }
        }
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(4))
        .build()
        .unwrap()
}

fn policy(attempts: u32) -> RetryPolicy {
    RetryPolicy {
        attempts,
        base_delay: Duration::ZERO,
    }
}

#[tokio::test]
async fn rate_limit_honors_retry_after_and_blocks_followup_network_requests() {
    let mut reply = Reply::new(429, "private rejection");
    reply.headers = "Retry-After: 120\r\n";
    let fixture = HttpFixture::start(vec![reply]).await;
    let endpoint = fixture.endpoint.clone();
    let connections = Semaphore::new(1);
    let cooldown = ApiCooldown::default();
    let result: Result<Value> = get_json_request(
        client().get(endpoint.clone()),
        &connections,
        &policy(3),
        &cooldown,
    )
    .await;
    assert_eq!(fixture.finish().await.len(), 1);
    assert_eq!(result.unwrap_err().http_status, Some(429));
    assert!(
        cooldown
            .0
            .lock()
            .unwrap()
            .unwrap()
            .saturating_duration_since(Instant::now())
            > Duration::from_secs(100)
    );
    // The server is gone: this still returns 429 without attempting a connection.
    let blocked: Result<Value> =
        get_json_request(client().get(endpoint), &connections, &policy(3), &cooldown).await;
    let error = blocked.unwrap_err();
    assert_eq!(error.http_status, Some(429));
    assert_eq!(error.operation, "wait before retrying Bilibili API");
    assert!(!error.retryable);
    assert_eq!(connections.available_permits(), 1);
    *cooldown.0.lock().unwrap() = Some(Instant::now() - Duration::from_secs(1));
    cooldown.check().unwrap();
}

#[test]
fn rate_limit_accepts_http_dates_and_defaults_without_exposing_headers() {
    let cooldown = ApiCooldown::default();
    let date = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(120));
    cooldown
        .record(Some(&header::HeaderValue::from_str(&date).unwrap()))
        .unwrap();
    assert!(
        cooldown
            .0
            .lock()
            .unwrap()
            .unwrap()
            .saturating_duration_since(Instant::now())
            > Duration::from_secs(100)
    );
    let fallback = ApiCooldown::default();
    fallback
        .record(Some(&header::HeaderValue::from_static(
            "synthetic-private-header",
        )))
        .unwrap();
    let error = fallback.check().unwrap_err();
    assert!(!serde_json::to_string(&error).unwrap().contains("synthetic"));
}

#[tokio::test]
async fn transient_status_and_interrupted_body_recover_the_same_get() {
    let mut interrupted = Reply::new(200, "{\"code\":0");
    interrupted.extra_length = 100;
    let fixture = HttpFixture::start(vec![
        Reply::new(503, "temporary"),
        interrupted,
        Reply::new(200, r#"{"code":0,"data":{"result":[]}}"#),
    ])
    .await;
    let connections = Semaphore::new(1);
    let result: Result<Value> = get_json_request(
        client().get(fixture.endpoint.clone()),
        &connections,
        &policy(3),
        &ApiCooldown::default(),
    )
    .await;
    let requests = fixture.finish().await;
    assert_eq!(result.unwrap()["code"], 0);
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| request == &requests[0]));
    assert_eq!(connections.available_permits(), 1);
}

#[tokio::test]
async fn only_the_selected_transient_http_statuses_retry() {
    for status in [408, 500, 502, 503, 504] {
        let fixture = HttpFixture::start(vec![
            Reply::new(status, "temporary"),
            Reply::new(200, r#"{"ok":true}"#),
        ])
        .await;
        let result: Result<Value> = get_json_request(
            client().get(fixture.endpoint.clone()),
            &Semaphore::new(1),
            &policy(3),
            &ApiCooldown::default(),
        )
        .await;
        let requests = fixture.finish().await;
        assert_eq!(result.unwrap()["ok"], true);
        assert_eq!(requests.len(), 2);
    }
}

#[tokio::test]
async fn exhaustion_retains_last_safe_failure_and_honors_the_attempt_limit() {
    for (configured, expected, code) in [
        (1, 1, ErrorCode::Transport),
        (2, 2, ErrorCode::RetryExhausted),
        (9, 3, ErrorCode::RetryExhausted),
    ] {
        let fixture = HttpFixture::start(
            (0..expected)
                .map(|_| Reply::new(503, "synthetic-secret-body"))
                .collect(),
        )
        .await;
        let connections = Semaphore::new(1);
        let result: Result<Value> = get_json_request(
            client().get(fixture.endpoint.clone()),
            &connections,
            &policy(configured),
            &ApiCooldown::default(),
        )
        .await;
        let requests = fixture.finish().await;
        let error = result.unwrap_err();
        assert_eq!(requests.len(), expected);
        assert_eq!(error.code, code);
        assert_eq!(error.operation, "request Bilibili API");
        assert_eq!(error.http_status, Some(503));
        assert!(error.retryable);
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains("synthetic"));
        assert!(!serialized.contains("fixture"));
        assert_eq!(connections.available_permits(), 1);
    }
    let delayed = RetryPolicy {
        attempts: 3,
        base_delay: Duration::from_millis(20),
    };
    assert_eq!(api_retry_delay(&delayed, 1), Duration::from_millis(20));
    assert_eq!(api_retry_delay(&delayed, 2), Duration::from_millis(40));
    assert_eq!(
        api_retry_delay(
            &RetryPolicy {
                attempts: 3,
                base_delay: Duration::MAX
            },
            1
        ),
        Duration::from_secs(2)
    );
}

#[tokio::test]
async fn rejections_malformed_json_and_semantic_failures_are_not_retried() {
    for status in [400, 401, 403, 404, 412, 429] {
        let fixture = HttpFixture::start(vec![Reply::new(status, "private-rejection")]).await;
        let result: Result<Value> = get_json_request(
            client().get(fixture.endpoint.clone()),
            &Semaphore::new(1),
            &policy(3),
            &ApiCooldown::default(),
        )
        .await;
        assert_eq!(fixture.finish().await.len(), 1);
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::Transport);
        assert_eq!(error.http_status, Some(status));
        assert!(!error.retryable);
    }
    for (body, code) in [
        ("not JSON", ErrorCode::InvalidInput),
        (r#"{"code":-101}"#, ErrorCode::LoginRequired),
        (r#"{"code":-10403}"#, ErrorCode::EntitlementRequired),
        (r#"{"code":-404}"#, ErrorCode::Unavailable),
        (r#"{"code":-500}"#, ErrorCode::Transport),
        (
            r#"{"code":-412,"data":{"v_voucher":"synthetic-private-voucher"}}"#,
            ErrorCode::VerificationRequired,
        ),
        (r#"{"code":0,"data":{"result":null}}"#, ErrorCode::Transport),
    ] {
        let fixture = HttpFixture::start(vec![Reply::new(200, body)]).await;
        let response: Result<Value> = get_json_request(
            client().get(fixture.endpoint.clone()),
            &Semaphore::new(1),
            &policy(3),
            &ApiCooldown::default(),
        )
        .await;
        let result = response.and_then(|value| parse_search_page(SearchCategory::Video, 2, &value));
        assert_eq!(fixture.finish().await.len(), 1);
        let error = result.unwrap_err();
        assert_eq!(error.code, code);
        assert!(!error.retryable);
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("synthetic-private-voucher"));
    }
}

#[tokio::test]
async fn a_delayed_successful_body_stays_pending_until_complete() {
    let (ready, started) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let mut reply = Reply::new(200, r#"{"code":0,"data":{"result":[]}}"#);
    reply.ready = Some(ready);
    reply.release = Some(released);
    let fixture = HttpFixture::start(vec![reply]).await;
    let endpoint = fixture.endpoint.clone();
    let connections = Arc::new(Semaphore::new(1));
    let owned_connections = connections.clone();
    let mut request = tokio::spawn(async move {
        get_json_request::<Value>(
            client().get(endpoint),
            &owned_connections,
            &policy(3),
            &ApiCooldown::default(),
        )
        .await
    });
    let started = tokio::time::timeout(Duration::from_secs(3), started).await;
    let early = tokio::time::timeout(Duration::from_millis(100), &mut request).await;
    let stayed_pending = early.is_err();
    let held = connections.available_permits();
    let _ = release.send(());
    let result = match early {
        Err(_) => tokio::time::timeout(Duration::from_secs(3), request)
            .await
            .unwrap()
            .unwrap(),
        Ok(result) => result.unwrap(),
    };
    let requests = fixture.finish().await;
    assert!(started.unwrap().is_ok());
    assert!(
        stayed_pending,
        "a partial successful response must remain pending"
    );
    assert_eq!(held, 0);
    assert_eq!(result.unwrap()["code"], 0);
    assert_eq!(requests.len(), 1);
    assert_eq!(connections.available_permits(), 1);
}

#[tokio::test]
async fn cancelling_a_body_read_releases_the_permit_and_has_no_later_retry() {
    let (ready, started) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let mut reply = Reply::new(200, r#"{"code":0,"data":{"result":[]}}"#);
    reply.ready = Some(ready);
    reply.release = Some(released);
    let fixture = HttpFixture::start(vec![reply]).await;
    let endpoint = fixture.endpoint.clone();
    let connections = Arc::new(Semaphore::new(1));
    let owned_connections = connections.clone();
    let request = tokio::spawn(async move {
        get_json_request::<Value>(
            client().get(endpoint),
            &owned_connections,
            &policy(3),
            &ApiCooldown::default(),
        )
        .await
    });
    let started = tokio::time::timeout(Duration::from_secs(3), started).await;
    request.abort();
    let result = request.await;
    let _ = release.send(());
    let requests = fixture.finish().await;
    assert!(started.unwrap().is_ok());
    assert!(result.unwrap_err().is_cancelled());
    assert_eq!(requests.len(), 1);
    assert!(connections.try_acquire().is_ok());
}

#[tokio::test]
async fn a_body_timeout_preserves_the_response_status_and_safe_stage() {
    let (release, released) = tokio::sync::oneshot::channel();
    let mut reply = Reply::new(200, r#"{"ok":true}"#);
    reply.release = Some(released);
    let fixture = HttpFixture::start(vec![reply]).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(250))
        .build()
        .unwrap();
    let result: Result<Value> = get_json_request(
        client.get(fixture.endpoint.clone()),
        &Semaphore::new(1),
        &policy(1),
        &ApiCooldown::default(),
    )
    .await;
    let _ = release.send(());
    assert_eq!(fixture.finish().await.len(), 1);
    let error = result.unwrap_err();
    assert_eq!(error.code, ErrorCode::Transport);
    assert_eq!(error.operation, "read Bilibili API response timed out");
    assert_eq!(error.http_status, Some(200));
    assert!(error.retryable);
}

#[tokio::test]
async fn backoff_releases_the_shared_connection_and_cancellation_ends_the_loop() {
    let fixture = HttpFixture::start(vec![Reply::new(503, "temporary")]).await;
    let endpoint = fixture.endpoint.clone();
    let connections = Arc::new(Semaphore::new(1));
    let owned_connections = connections.clone();
    let request = tokio::spawn(async move {
        get_json_request::<Value>(
            client().get(endpoint),
            &owned_connections,
            &RetryPolicy {
                attempts: 3,
                base_delay: Duration::from_secs(60),
            },
            &ApiCooldown::default(),
        )
        .await
    });
    let requests = fixture.finish().await;
    // Another resource can acquire the only permit while this GET backs off.
    let other = tokio::time::timeout(Duration::from_secs(1), connections.acquire()).await;
    request.abort();
    let stopped = request.await;
    assert_eq!(requests.len(), 1);
    let other = other.unwrap().unwrap();
    assert!(stopped.unwrap_err().is_cancelled());
    drop(other);
    assert_eq!(connections.available_permits(), 1);

    let fixture = HttpFixture::start(vec![Reply::new(200, r#"{"ok":true}"#)]).await;
    let response: Value = get_json_request(
        client().get(fixture.endpoint.clone()),
        &connections,
        &policy(3),
        &ApiCooldown::default(),
    )
    .await
    .unwrap();
    assert_eq!(fixture.finish().await.len(), 1);
    assert_eq!(response["ok"], true);
}
