//! What the newer operations send: keys, queries, bodies and their pages.

mod common;

use common::{ContractMock, ID};
use futures_util::TryStreamExt;
use serde_json::{Value, json};

#[tokio::test]
async fn an_event_is_sent_once_with_a_key() {
    let mock = ContractMock::start().await;
    mock.error("POST", "/events/send", 503, "internal_server_error", "Down")
        .header("retry-after", "0");
    mock.client()
        .events()
        .send(&json!({ "event": "signed.up", "email": "ana@example.com" }))
        .await
        .unwrap();
    let calls = mock.calls();
    assert_eq!(calls.len(), 2);
    let key = calls[0].header("idempotency-key").unwrap();
    assert_eq!(calls[1].header("idempotency-key"), Some(key));
    mock.assert_clean();
}

#[tokio::test]
async fn an_event_name_is_one_path_segment() {
    let mock = ContractMock::start().await;
    mock.client()
        .events()
        .get("orders/paid 100%")
        .await
        .unwrap();
    assert_eq!(mock.last().path_params["id_or_name"], "orders/paid 100%");
    mock.assert_clean();
}

#[tokio::test]
async fn a_domain_is_suppressed_with_a_note_and_listed_by_kind() {
    let mock = ContractMock::start().await;
    let client = mock.client();
    let body = json!({ "domain": "example.org", "note": "asked to stop" });
    client.suppressions().create(&body).await.unwrap();
    assert_eq!(mock.last().body, body);
    let body = json!({ "domains": ["example.org"] });
    client.suppressions().batch().add(&body).await.unwrap();
    assert_eq!(mock.last().body, body);
    client.suppressions().list().kind("domain").await.unwrap();
    assert_eq!(mock.last().query("kind"), Some("domain"));
    mock.assert_clean();
}

#[tokio::test]
async fn broadcast_recipients_page_with_their_filter() {
    let mock = ContractMock::start().await;
    let pointer =
        "/paths/~1broadcasts~1{id}~1recipients/get/responses/200/content/application~1json/schema";
    let schema = common::SPEC.pointer(pointer).unwrap();
    let page = |id: &str, has_more: bool| -> Value {
        let mut page = common::example(schema, true);
        page["data"][0]["id"] = json!(id);
        page["has_more"] = json!(has_more);
        page
    };
    let first = "0b6f5a2e-7c1d-4e8a-9f3b-2d4c6e8a0b1c";
    let second = "1c7a6b3f-8d2e-4f9b-a04c-3e5d7f9b1c2d";
    for (id, has_more) in [(first, true), (second, false)] {
        mock.respond(
            "GET",
            "/broadcasts/{id}/recipients",
            wiremock::ResponseTemplate::new(200).set_body_json(page(id, has_more)),
        );
    }
    let items: Vec<Value> = mock
        .client()
        .broadcasts()
        .recipients(ID)
        .r#type("bounced")
        .bounce_type("permanent")
        .into_stream()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(items.len(), 2);
    let calls = mock.calls();
    assert_eq!(calls[1].query("after"), Some(first));
    assert_eq!(calls[1].query("type"), Some("bounced"));
    assert_eq!(calls[1].query("bounce_type"), Some("permanent"));
    mock.assert_clean();
}
