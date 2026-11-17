//! Retries: what is retried, how often, and with which key.

mod common;

use std::time::{Duration, Instant};

use billionsend::Error;
use common::{ContractMock, ID};
use serde_json::json;
use wiremock::ResponseTemplate;

fn send_body() -> serde_json::Value {
    json!({ "from": "hello@acme.com", "to": "user@example.com", "subject": "Hi", "text": "Hi" })
}

#[tokio::test]
async fn a_get_is_retried_on_429_and_5xx() {
    let mock = ContractMock::start().await;
    mock.error("GET", "/domains", 429, "rate_limit_exceeded", "Slow down")
        .header("retry-after", "0");
    mock.error(
        "GET",
        "/domains",
        503,
        "internal_server_error",
        "Unavailable",
    )
    .header("retry-after", "0");
    mock.client().domains().list().await.unwrap();
    assert_eq!(mock.calls().len(), 3);
    mock.assert_clean();
}

#[tokio::test]
async fn retries_stop_at_max_retries() {
    let mock = ContractMock::start().await;
    for _ in 0..3 {
        mock.error("GET", "/domains", 500, "internal_server_error", "Boom")
            .header("retry-after", "0");
    }
    let error = mock
        .client()
        .domains()
        .list()
        .max_retries(1)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Server(_)));
    assert_eq!(mock.calls().len(), 2);
}

#[tokio::test]
async fn a_post_without_a_key_is_not_retried() {
    let mock = ContractMock::start().await;
    mock.error("POST", "/domains", 500, "internal_server_error", "Boom")
        .header("retry-after", "0");
    let error = mock
        .client()
        .domains()
        .create(&json!({ "name": "example.com" }))
        .await
        .unwrap_err();
    assert_eq!(error.status_code(), Some(500));
    assert_eq!(mock.calls().len(), 1);
}

#[tokio::test]
async fn a_send_is_retried_with_the_same_key() {
    let mock = ContractMock::start().await;
    mock.error("POST", "/emails", 500, "internal_server_error", "Boom")
        .header("retry-after", "0");
    mock.error("POST", "/emails", 429, "rate_limit_exceeded", "Slow down")
        .header("retry-after", "0");
    mock.client().emails().send(&send_body()).await.unwrap();
    let keys: Vec<String> = mock
        .calls()
        .iter()
        .map(|c| c.header("idempotency-key").unwrap().to_owned())
        .collect();
    assert_eq!(keys.len(), 3);
    assert!(keys.iter().all(|k| *k == keys[0]), "{keys:?}");
    mock.assert_clean();
}

#[tokio::test]
async fn concurrent_idempotent_requests_is_retried_only_with_a_key() {
    let mock = ContractMock::start().await;
    mock.error(
        "POST",
        "/emails",
        409,
        "concurrent_idempotent_requests",
        "In progress",
    )
    .header("retry-after", "0");
    mock.client().emails().send(&send_body()).await.unwrap();
    assert_eq!(mock.calls().len(), 2);

    mock.clear();
    mock.error(
        "POST",
        "/emails",
        409,
        "invalid_idempotent_request",
        "Other payload",
    )
    .header("retry-after", "0");
    let error = mock.client().emails().send(&send_body()).await.unwrap_err();
    assert!(matches!(error, Error::Conflict(_)));
    assert_eq!(mock.calls().len(), 1, "other 409s are final");

    mock.clear();
    mock.error(
        "POST",
        "/domains",
        409,
        "concurrent_idempotent_requests",
        "In progress",
    )
    .header("retry-after", "0");
    let error = mock
        .client()
        .domains()
        .create(&json!({ "name": "example.com" }))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Conflict(_)));
    assert_eq!(mock.calls().len(), 1, "no key, no retry");
}

#[tokio::test]
async fn a_long_retry_after_is_not_waited_for() {
    let mock = ContractMock::start().await;
    mock.error("GET", "/domains", 429, "rate_limit_exceeded", "Later")
        .header("retry-after", "61");
    let started = Instant::now();
    let error = mock.client().domains().list().await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(error.retry_after(), Some(61.0));
    assert_eq!(mock.calls().len(), 1);
}

#[tokio::test]
async fn retry_after_is_honoured() {
    let mock = ContractMock::start().await;
    mock.error("GET", "/emails/{id}", 429, "rate_limit_exceeded", "Wait")
        .header("retry-after", "1");
    let started = Instant::now();
    mock.client().emails().get(ID).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(1000));
    assert_eq!(mock.calls().len(), 2);
}

#[tokio::test]
async fn a_timeout_is_retried_for_a_get() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/domains",
        ResponseTemplate::new(200)
            .set_body_json(json!({}))
            .set_delay(Duration::from_millis(400)),
    );
    let client = mock
        .builder()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    client.domains().list().await.unwrap();
    assert_eq!(mock.calls().len(), 2);
}

#[tokio::test]
async fn a_4xx_is_final() {
    let mock = ContractMock::start().await;
    mock.error("GET", "/domains", 400, "validation_error", "Bad");
    let error = mock.client().domains().list().await.unwrap_err();
    assert!(matches!(error, Error::Validation(_)));
    assert_eq!(mock.calls().len(), 1);
}
