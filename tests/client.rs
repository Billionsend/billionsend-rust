//! The client: configuration, headers, paths, idempotency keys and errors.

mod common;

use std::time::Duration;

use billionsend::{BillionSend, Error};
use common::{API_KEY, ContractMock, ID, INBOX};
use serde_json::json;

fn send_body() -> serde_json::Value {
    json!({
        "from": "Acme <hello@acme.com>",
        "to": "user@example.com",
        "subject": "Hello",
        "html": "<p>Hi</p>",
    })
}

#[tokio::test]
async fn sends_the_key_the_user_agent_and_json() {
    let mock = ContractMock::start().await;
    let email = mock.client().emails().send(&send_body()).await.unwrap();
    assert!(email["id"].is_string());
    let call = mock.last();
    assert_eq!(
        call.header("authorization"),
        Some(format!("Bearer {API_KEY}").as_str())
    );
    assert_eq!(call.header("accept"), Some("application/json"));
    assert_eq!(call.header("content-type"), Some("application/json"));
    assert_eq!(
        call.header("user-agent"),
        Some(concat!("billionsend-rust/", env!("CARGO_PKG_VERSION")))
    );
    assert_eq!(call.body, send_body());
    mock.assert_clean();
}

#[tokio::test]
async fn without_a_key_nothing_is_sent() {
    let mock = ContractMock::start().await;
    let client = BillionSend::builder().base_url(mock.uri()).build().unwrap();
    let error = client.domains().list().await.unwrap_err();
    assert!(matches!(error, Error::Authentication(_)));
    assert_eq!(error.name(), "missing_api_key");
    assert_eq!(error.status_code(), None);
    assert!(mock.calls().is_empty());
}

#[tokio::test]
async fn agent_signup_goes_without_a_key() {
    let mock = ContractMock::start().await;
    let client = BillionSend::builder().base_url(mock.uri()).build().unwrap();
    let body = json!({ "human_email": "owner@example.com", "name": "Support agent" });
    client.agent().signup(&body).await.unwrap();
    assert_eq!(mock.last().header("authorization"), None);
    // A keyed client still sends no key there.
    mock.client().agent().signup(&body).await.unwrap();
    assert_eq!(mock.last().header("authorization"), None);
    mock.assert_clean();
}

#[tokio::test]
async fn path_parameters_are_escaped_and_required() {
    let mock = ContractMock::start().await;
    let client = mock.client();
    client.inboxes().get(INBOX).await.unwrap();
    assert_eq!(mock.last().path_params["inbox_id"], INBOX);
    client.inboxes().get("a/b?c").await.unwrap();
    assert_eq!(mock.last().path_params["inbox_id"], "a/b?c");

    mock.clear();
    let error = client.emails().get("").await.unwrap_err();
    assert!(matches!(error, Error::Application(_)));
    assert_eq!(error.message(), "Missing path parameter \"id\"");
    assert!(mock.calls().is_empty(), "nothing is sent");
    mock.assert_clean();
}

#[tokio::test]
async fn headers_from_the_client_and_the_request() {
    let mock = ContractMock::start().await;
    let client = mock
        .builder()
        .header("X-Trace", "client")
        .header("X-Other", "1")
        .build()
        .unwrap();
    client
        .domains()
        .list()
        .header("x-trace", "request")
        .await
        .unwrap();
    let call = mock.last();
    assert_eq!(call.header("x-trace"), Some("request"));
    assert_eq!(call.header("x-other"), Some("1"));
    mock.assert_clean();
}

#[tokio::test]
async fn sending_operations_always_carry_an_idempotency_key() {
    let mock = ContractMock::start().await;
    let client = mock.client();
    client.emails().send(&send_body()).await.unwrap();
    let first = mock.last().header("idempotency-key").unwrap().to_owned();
    assert_eq!(uuid::Uuid::parse_str(&first).unwrap().get_version_num(), 4);
    client.emails().send(&send_body()).await.unwrap();
    let second = mock.last().header("idempotency-key").unwrap().to_owned();
    assert_ne!(first, second, "a new key per call");

    client
        .emails()
        .send(&send_body())
        .idempotency_key("welcome/42")
        .await
        .unwrap();
    assert_eq!(mock.last().header("idempotency-key"), Some("welcome/42"));

    client
        .emails()
        .send(&send_body())
        .header("Idempotency-Key", "from-header")
        .await
        .unwrap();
    assert_eq!(mock.last().header("idempotency-key"), Some("from-header"));

    client.domains().list().await.unwrap();
    assert_eq!(
        mock.last().header("idempotency-key"),
        None,
        "only the sending operations"
    );
    mock.assert_clean();
}

#[tokio::test]
async fn raw_and_typed_answers() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/emails/{id}",
        wiremock::ResponseTemplate::new(200)
            .set_body_json(common::example(
                common::SPEC
                    .pointer("/components/schemas/GetEmailResponse")
                    .unwrap_or(&json!({"type": "object"})),
                true,
            ))
            .insert_header("x-request-id", "req_123"),
    );
    let raw = mock.client().emails().get(ID).raw().await.unwrap();
    assert_eq!(raw.status, 200);
    assert_eq!(raw.request_id(), Some("req_123"));

    #[derive(serde::Deserialize)]
    struct Email {
        id: String,
    }
    let typed: Email = mock.client().emails().get(ID).json().await.unwrap();
    assert!(!typed.id.is_empty());
    mock.assert_clean();
}

#[tokio::test]
async fn a_timeout_is_its_own_error() {
    let mock = ContractMock::start().await;
    mock.delay("POST", "/domains", Duration::from_millis(500));
    let client = mock
        .builder()
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    let error = client
        .domains()
        .create(&json!({ "name": "example.com" }))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Timeout(_)), "{error:?}");
    assert_eq!(error.status_code(), None);
    assert_eq!(mock.calls().len(), 1, "a POST without a key is not retried");
}

#[tokio::test]
async fn a_refused_connection_is_a_connection_error() {
    let client = BillionSend::builder()
        .api_key(API_KEY)
        .base_url("http://127.0.0.1:9")
        .max_retries(0)
        .build()
        .unwrap();
    let error = client.domains().list().await.unwrap_err();
    assert!(matches!(error, Error::Connection(_)), "{error:?}");
    assert!(error.message().starts_with("Network error"));
}

#[tokio::test]
async fn the_mock_refuses_what_the_contract_does_not_declare() {
    let mock = ContractMock::start().await;
    let error = mock
        .client()
        .emails()
        .send(&json!({ "from": 1 }))
        .max_retries(0)
        .await
        .unwrap_err();
    assert_eq!(error.status_code(), Some(599));
    let violations = mock.take_violations();
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("request body"), "{violations:?}");
}

#[test]
fn a_zero_timeout_or_a_bad_header_does_not_build() {
    assert!(
        BillionSend::builder()
            .timeout(Duration::ZERO)
            .build()
            .is_err()
    );
    assert!(
        BillionSend::builder()
            .header("bad header", "x")
            .build()
            .is_err()
    );
    assert!(
        BillionSend::builder()
            .header("x-ok", "line\nbreak")
            .build()
            .is_err()
    );
}

#[test]
fn debug_output_hides_the_key() {
    let client = BillionSend::new("bs_secret_value");
    let text = format!("{client:?}");
    assert!(!text.contains("bs_secret_value"), "{text}");
    assert!(text.contains("<set>"));
}

#[tokio::test]
async fn a_success_without_a_body_is_null() {
    let mock = ContractMock::start().await;
    for status in [204, 200] {
        mock.respond(
            "DELETE",
            "/domains/{id}",
            wiremock::ResponseTemplate::new(status),
        );
    }
    let client = mock.client();
    assert!(client.domains().delete(ID).await.unwrap().is_null());
    let raw = client.domains().delete(ID).raw().await.unwrap();
    assert_eq!(raw.status, 200);
    assert!(raw.body.is_null());
}

#[tokio::test]
async fn a_dot_path_parameter_sends_nothing() {
    let mock = ContractMock::start().await;
    for id in [".", ".."] {
        let error = mock.client().emails().get(id).await.unwrap_err();
        assert!(matches!(error, Error::Application(_)), "{error:?}");
        assert_eq!(
            error.message(),
            format!("Invalid path parameter \"id\": \"{id}\"")
        );
    }
    assert!(mock.calls().is_empty());
}

#[tokio::test]
async fn a_header_http_cannot_carry_is_refused_without_its_value() {
    let mock = ContractMock::start().await;
    for key in ["welcome/订单-1", "line\nbreak"] {
        let error = mock
            .client()
            .emails()
            .send(&send_body())
            .idempotency_key(key)
            .await
            .unwrap_err();
        assert_eq!(error.name(), "invalid_header");
        assert!(error.message().contains("idempotency-key"));
        assert!(!error.message().contains(key));
    }
    let error = BillionSend::builder()
        .api_key("bs_bad\nkey")
        .base_url(mock.uri())
        .build()
        .unwrap()
        .domains()
        .list()
        .await
        .unwrap_err();
    assert_eq!(error.name(), "invalid_header");
    assert!(!error.message().contains("bs_bad"));
    assert!(mock.calls().is_empty());
}

#[tokio::test]
async fn the_key_is_read_without_the_whitespace_around_it() {
    let mock = ContractMock::start().await;
    let client = BillionSend::builder()
        .api_key(format!("  {API_KEY}\n"))
        .base_url(mock.uri())
        .build()
        .unwrap();
    client.domains().list().await.unwrap();
    mock.assert_clean();
}
