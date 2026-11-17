//! Each status maps to its error variant, with the API's name, message and request id.

mod common;

use billionsend::Error;
use common::ContractMock;
use wiremock::ResponseTemplate;

type IsVariant = fn(&Error) -> bool;

#[tokio::test]
async fn statuses_map_to_variants() {
    let cases: [(u16, &str, IsVariant); 8] = [
        (400, "validation_error", |e| {
            matches!(e, Error::Validation(_))
        }),
        (422, "validation_error", |e| {
            matches!(e, Error::Validation(_))
        }),
        (401, "invalid_api_key", |e| {
            matches!(e, Error::Authentication(_))
        }),
        (403, "restricted_api_key", |e| {
            matches!(e, Error::PermissionDenied(_))
        }),
        (404, "not_found", |e| matches!(e, Error::NotFound(_))),
        (409, "invalid_idempotent_request", |e| {
            matches!(e, Error::Conflict(_))
        }),
        (429, "rate_limit_exceeded", |e| {
            matches!(e, Error::RateLimit(_))
        }),
        (500, "internal_server_error", |e| {
            matches!(e, Error::Server(_))
        }),
    ];
    let mock = ContractMock::start().await;
    let client = mock.builder().max_retries(0).build().unwrap();
    for (status, name, is_variant) in cases {
        mock.error("GET", "/domains", status, name, "It failed")
            .header("x-request-id", "req_1");
        let error = client.domains().list().await.unwrap_err();
        assert!(is_variant(&error), "{status}: {error:?}");
        assert_eq!(error.status_code(), Some(status));
        assert_eq!(error.name(), name);
        assert_eq!(error.message(), "It failed");
        assert_eq!(error.request_id(), Some("req_1"));
        assert_eq!(
            error.to_string(),
            format!("It failed ({status} {name}) [request req_1]")
        );
    }
}

#[tokio::test]
async fn rate_limits_carry_retry_after() {
    let mock = ContractMock::start().await;
    mock.error("GET", "/domains", 429, "rate_limit_exceeded", "Slow")
        .header("retry-after", "120");
    let error = mock.client().domains().list().await.unwrap_err();
    assert!(matches!(error, Error::RateLimit(_)));
    assert_eq!(error.retry_after(), Some(120.0));
}

#[tokio::test]
async fn an_answer_without_the_error_shape() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/domains",
        ResponseTemplate::new(418).set_body_string("<html>teapot</html>"),
    );
    let error = mock.client().domains().list().await.unwrap_err();
    assert!(matches!(error, Error::Api(_)));
    assert_eq!(error.name(), "application_error");
    assert_eq!(
        error.message(),
        "Unexpected 418 I'm a teapot response: <html>teapot</html>"
    );

    mock.respond("GET", "/domains", ResponseTemplate::new(502));
    let error = mock
        .client()
        .domains()
        .list()
        .max_retries(0)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Server(_)));
    assert_eq!(error.message(), "Unexpected 502 Bad Gateway response");
}

#[tokio::test]
async fn a_success_that_is_not_json() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/domains",
        ResponseTemplate::new(200).set_body_string("not json"),
    );
    let error = mock.client().domains().list().await.unwrap_err();
    assert!(matches!(error, Error::Api(_)));
    assert_eq!(error.status_code(), Some(200));
    assert!(
        error
            .message()
            .starts_with("The response body is not the expected JSON")
    );
}

#[tokio::test]
async fn a_5xx_carries_retry_after_too() {
    let mock = ContractMock::start().await;
    mock.error("GET", "/domains", 503, "internal_server_error", "Busy")
        .header("retry-after", "7");
    let error = mock
        .client()
        .domains()
        .list()
        .max_retries(0)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Server(_)));
    assert_eq!(error.retry_after(), Some(7.0));
}
