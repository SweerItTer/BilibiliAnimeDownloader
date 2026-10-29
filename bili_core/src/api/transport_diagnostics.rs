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
        secret,
    );
}

fn assert_safe_transport_error(error: &CoreError, operation: &str, secret: &str) {
    assert_eq!(error.code, ErrorCode::Transport);
    assert_eq!(error.operation, operation);
    assert!(!error.retryable);
    assert_eq!(error.http_status, None);
    assert!(std::error::Error::source(error).is_none());

    let rendered = [
        error.to_string(),
        format!("{error:?}"),
        serde_json::to_string(error).unwrap(),
    ];
    assert!(rendered.iter().all(|text| !text.contains(secret)));
}
