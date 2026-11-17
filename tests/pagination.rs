//! `into_stream()` walks cursor-paginated lists.

mod common;

use billionsend::Error;
use common::ContractMock;
use futures_util::{StreamExt, TryStreamExt};
use serde_json::{Value, json};

fn page(ids: &[&str], has_more: bool) -> Value {
    let item = common::example(
        common::SPEC
            .pointer("/paths/~1domains/get/responses/200/content/application~1json/schema")
            .unwrap(),
        true,
    )["data"][0]
        .clone();
    let data: Vec<Value> = ids
        .iter()
        .map(|id| {
            let mut item = item.clone();
            item["id"] = json!(id);
            item
        })
        .collect();
    json!({ "object": "list", "has_more": has_more, "data": data })
}

#[tokio::test]
async fn walks_forward_from_the_start() {
    let mock = ContractMock::start().await;
    mock.respond_json("GET", "/domains", 200, page(&["a", "b"], true));
    mock.respond_json("GET", "/domains", 200, page(&["c"], false));
    let items: Vec<Value> = mock
        .client()
        .domains()
        .list()
        .limit(2)
        .into_stream()
        .try_collect()
        .await
        .unwrap();
    let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    let calls = mock.calls();
    assert_eq!(calls[0].query("after"), None);
    assert_eq!(calls[1].query("after"), Some("b"));
    assert_eq!(calls[1].query("limit"), Some("2"));
    mock.assert_clean();
}

#[tokio::test]
async fn walks_backward_from_before() {
    let mock = ContractMock::start().await;
    mock.respond_json("GET", "/domains", 200, page(&["x", "y"], true));
    mock.respond_json("GET", "/domains", 200, page(&["v", "w"], false));
    let items: Vec<Value> = mock
        .client()
        .domains()
        .list()
        .before("z")
        .after("ignored")
        .into_stream()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(items.len(), 4);
    let calls = mock.calls();
    assert_eq!(calls[0].query("before"), Some("z"));
    assert_eq!(calls[0].query("after"), None);
    assert_eq!(calls[1].query("before"), Some("x"));
    mock.assert_clean();
}

#[tokio::test]
async fn an_empty_page_ends_the_walk() {
    let mock = ContractMock::start().await;
    mock.respond_json("GET", "/domains", 200, page(&[], true));
    let items: Vec<Value> = mock
        .client()
        .domains()
        .list()
        .into_stream()
        .try_collect()
        .await
        .unwrap();
    assert!(items.is_empty());
    assert_eq!(mock.calls().len(), 1);
}

#[tokio::test]
async fn a_failed_page_ends_the_stream_with_its_error() {
    let mock = ContractMock::start().await;
    mock.respond_json("GET", "/domains", 200, page(&["a"], true));
    mock.error("GET", "/domains", 401, "invalid_api_key", "Revoked");
    let mut stream = mock.client().domains().list().into_stream();
    assert_eq!(stream.next().await.unwrap().unwrap()["id"], "a");
    let error = stream.next().await.unwrap().unwrap_err();
    assert!(matches!(error, Error::Authentication(_)));
    assert!(stream.next().await.is_none());
}
