//! Inbox event streams: parsing, reconnects with Last-Event-ID, gaps, ends and give-ups.

mod common;

use std::time::Duration;

use billionsend::sse::EventParser;
use billionsend::{Error, InboxStreamItem};
use common::{ContractMock, INBOX};
use futures_util::StreamExt;
use serde_json::json;
use wiremock::ResponseTemplate;

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "text/event-stream")
}

fn event(id: &str, name: &str) -> String {
    format!(
        "id: {id}\nevent: {name}\ndata: {{\"type\":\"{name}\",\"inbox_id\":\"in_1\",\"data\":{{\"thread_id\":\"t_{id}\"}}}}\n\n"
    )
}

fn ids(items: &[InboxStreamItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| match item {
            InboxStreamItem::Event(e) => e.id.clone(),
            InboxStreamItem::Gap(g) => format!("gap:{}", g.reason),
        })
        .collect()
}

#[test]
fn the_parser_follows_the_rules() {
    let mut parser = EventParser::new(None);
    let mut events = Vec::new();
    // A BOM, CRLF and CR line ends, a comment, a multi-line data field, a
    // field without a colon, a chunk boundary inside "\r\n" and inside a line.
    for chunk in [
        &b"\xEF\xBB"[..],
        b"\xBF: comment\r\nid: 7\r",
        b"\nevent: one\rdata: {\"a\":",
        b"1}\n\ndata: line 1\ndata: line 2\ndata\n\n",
        b"retry: 2500\nid: 8\nevent: two\ndata: x\n\n",
        b"event: dropped\ndata: never ends",
    ] {
        events.extend(parser.feed(chunk));
    }
    events.extend(parser.finish());
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].id.as_deref(), Some("7"));
    assert_eq!(events[0].event, "one");
    assert_eq!(events[0].data, json!({ "a": 1 }));
    assert_eq!(events[1].event, "message");
    assert_eq!(events[1].raw, "line 1\nline 2\n");
    assert_eq!(
        events[1].id.as_deref(),
        Some("7"),
        "the position carries over"
    );
    assert_eq!(events[2].id.as_deref(), Some("8"));
    assert_eq!(events[2].data, json!("x"));
    assert_eq!(parser.take_retry(), Some(2500));
}

#[tokio::test]
async fn reconnects_with_last_event_id() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&(event("1", "inbox.email.received") + &event("2", "inbox.thread.created"))),
    );
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("3", "inbox.email.received")),
    );
    let stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .types("inbox.email.received,inbox.thread.created")
        .reconnect_delay(Duration::from_millis(5))
        .await
        .unwrap();
    let items: Vec<InboxStreamItem> = stream.take(3).map(Result::unwrap).collect().await;
    assert_eq!(ids(&items), ["1", "2", "3"]);
    let InboxStreamItem::Event(first) = &items[0] else {
        panic!()
    };
    assert_eq!(first.event, "inbox.email.received");
    assert_eq!(first.data["data"]["thread_id"], "t_1");
    let calls = mock.calls();
    assert_eq!(calls[0].header("accept"), Some("text/event-stream"));
    assert_eq!(calls[0].header("last-event-id"), None);
    assert_eq!(
        calls[0].query("types"),
        Some("inbox.email.received,inbox.thread.created")
    );
    assert_eq!(calls[1].header("last-event-id"), Some("2"));
    assert_eq!(
        calls[1].query("types"),
        Some("inbox.email.received,inbox.thread.created")
    );
    mock.assert_clean();
}

#[tokio::test]
async fn resumes_from_a_given_position() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("42", "inbox.email.received")),
    );
    let mut stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .last_event_id("41")
        .reconnect(false)
        .await
        .unwrap();
    assert_eq!(stream.last_event_id().as_deref(), Some("41"));
    assert!(matches!(
        stream.next().await,
        Some(Ok(InboxStreamItem::Event(_)))
    ));
    assert_eq!(stream.last_event_id().as_deref(), Some("42"));
    assert!(stream.next().await.is_none(), "no reconnect");
    assert_eq!(mock.calls()[0].header("last-event-id"), Some("41"));
    mock.assert_clean();
}

#[tokio::test]
async fn a_gap_is_handed_out_once_per_position() {
    let mock = ContractMock::start().await;
    let gap = "event: stream.gap\ndata: {\"reason\":\"expired\"}\n\n";
    mock.respond("GET", "/inboxes/events", sse(gap));
    mock.respond("GET", "/inboxes/events", sse(gap));
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&(gap.to_owned() + &event("9", "inbox.email.received"))),
    );
    mock.respond("GET", "/inboxes/events", sse(gap));
    let stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .last_event_id("1")
        .reconnect_delay(Duration::from_millis(5))
        .await
        .unwrap();
    let items: Vec<InboxStreamItem> = stream.take(3).map(Result::unwrap).collect().await;
    assert_eq!(ids(&items), ["gap:expired", "9", "gap:expired"]);
    let calls = mock.calls();
    let positions: Vec<Option<&str>> = calls.iter().map(|c| c.header("last-event-id")).collect();
    assert_eq!(positions, [Some("1"), Some("1"), Some("1"), Some("9")]);
}

#[tokio::test]
async fn one_inbox_stream_ends_when_the_inbox_is_gone() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/inboxes/{inbox_id}/events",
        sse(&(event("1", "inbox.email.received")
            + &event("2", "inbox.deleted")
            + &event("3", "inbox.email.received"))),
    );
    let stream = mock
        .client()
        .inboxes()
        .events()
        .stream_inbox(INBOX)
        .await
        .unwrap();
    let items: Vec<InboxStreamItem> = stream.map(Result::unwrap).collect().await;
    assert_eq!(ids(&items), ["1", "2"]);
    assert_eq!(mock.calls().len(), 1, "no reconnect after inbox.deleted");
    assert_eq!(mock.last().path_params["inbox_id"], INBOX);
}

#[tokio::test]
async fn gives_up_after_max_reconnects_that_never_answer() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("1", "inbox.email.received")),
    );
    // Every reconnect waits past the connection timeout: no answer at all.
    for _ in 0..5 {
        mock.respond(
            "GET",
            "/inboxes/events",
            sse(&event("9", "inbox.email.received")).set_delay(Duration::from_millis(500)),
        );
    }
    let stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .timeout(Duration::from_millis(50))
        .max_reconnects(2)
        .reconnect_delay(Duration::from_millis(5))
        .await
        .unwrap();
    let items: Vec<_> = stream.collect().await;
    assert_eq!(items.len(), 2);
    let error = items[1].as_ref().unwrap_err();
    assert!(matches!(error, Error::Connection(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "The event stream disconnected; gave up after 2 reconnects"
    );
    assert_eq!(
        mock.calls().len(),
        3,
        "the first connection and two reconnects, each tried once"
    );
}

#[tokio::test]
async fn reconnects_refused_by_the_api_end_with_the_api_error() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("1", "inbox.email.received")),
    );
    mock.error(
        "GET",
        "/inboxes/events",
        503,
        "internal_server_error",
        "Down",
    );
    for _ in 0..3 {
        mock.error(
            "GET",
            "/inboxes/events",
            429,
            "too_many_streams",
            "Too many open streams",
        )
        .header("retry-after", "0");
    }
    let stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .max_reconnects(3)
        .reconnect_delay(Duration::from_millis(5))
        .await
        .unwrap();
    let items: Vec<_> = stream.collect().await;
    assert_eq!(items.len(), 2);
    let error = items[1].as_ref().unwrap_err();
    assert!(matches!(error, Error::RateLimit(_)), "{error:?}");
    assert_eq!(error.name(), "too_many_streams");
    assert_eq!(error.status_code(), Some(429));
    assert_eq!(mock.calls().len(), 4);
}

#[tokio::test]
async fn a_refused_reconnect_waits_as_retry_after_asks() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("1", "inbox.email.received")),
    );
    mock.error(
        "GET",
        "/inboxes/events",
        429,
        "too_many_streams",
        "Too many open streams",
    )
    .header("retry-after", "1");
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("2", "inbox.email.received")),
    );
    let mut stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .reconnect_delay(Duration::from_millis(5))
        .await
        .unwrap();
    assert!(matches!(stream.next().await, Some(Ok(_))));
    let started = std::time::Instant::now();
    let item = stream.next().await;
    assert!(
        matches!(item, Some(Ok(InboxStreamItem::Event(ref e))) if e.id == "2"),
        "{item:?}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(1000),
        "waited {:?}",
        started.elapsed()
    );
    assert_eq!(mock.calls().len(), 3);
}

#[tokio::test]
async fn a_retry_after_over_a_minute_ends_the_stream_at_once() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("1", "inbox.email.received")),
    );
    mock.error(
        "GET",
        "/inboxes/events",
        429,
        "too_many_streams",
        "Too many open streams",
    )
    .header("retry-after", "120");
    let stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .reconnect_delay(Duration::from_millis(5))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let items: Vec<_> = tokio::time::timeout(Duration::from_secs(5), stream.collect())
        .await
        .expect("the stream ends at once");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(items.len(), 2);
    let error = items[1].as_ref().unwrap_err();
    assert!(matches!(error, Error::RateLimit(_)), "{error:?}");
    assert_eq!(error.retry_after(), Some(120.0));
    assert_eq!(mock.calls().len(), 2, "no reconnect after the refusal");
}

#[tokio::test]
async fn an_error_a_retry_cannot_fix_ends_the_stream() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("1", "inbox.email.received")),
    );
    mock.error("GET", "/inboxes/events", 401, "invalid_api_key", "Revoked");
    let stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .reconnect_delay(Duration::from_millis(5))
        .await
        .unwrap();
    let items: Vec<_> = stream.collect().await;
    assert_eq!(items.len(), 2);
    assert!(matches!(
        items[1].as_ref().unwrap_err(),
        Error::Authentication(_)
    ));
}

#[tokio::test]
async fn the_first_connection_fails_like_a_request() {
    let mock = ContractMock::start().await;
    mock.error(
        "GET",
        "/inboxes/events",
        403,
        "restricted_api_key",
        "Team-wide key needed",
    );
    let error = mock.client().inboxes().events().stream().await.unwrap_err();
    assert!(matches!(error, Error::PermissionDenied(_)));

    mock.respond(
        "GET",
        "/inboxes/events",
        ResponseTemplate::new(200).set_body_json(json!({})),
    );
    let error = mock.client().inboxes().events().stream().await.unwrap_err();
    assert_eq!(
        error.message(),
        "Expected an event stream, got application/json"
    );

    mock.error(
        "GET",
        "/inboxes/events",
        503,
        "internal_server_error",
        "Down",
    )
    .header("retry-after", "0");
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("1", "inbox.email.received")),
    );
    let mut stream = mock.client().inboxes().events().stream().await.unwrap();
    assert!(
        matches!(stream.next().await, Some(Ok(InboxStreamItem::Event(_)))),
        "the first connection is retried"
    );
}

#[tokio::test]
async fn a_retry_field_sets_the_reconnect_delay() {
    let mock = ContractMock::start().await;
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&("retry: 10\n".to_owned() + &event("1", "inbox.email.received"))),
    );
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("2", "inbox.email.received")),
    );
    let stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .reconnect_delay(Duration::from_secs(30))
        .await
        .unwrap();
    let items = tokio::time::timeout(Duration::from_secs(5), stream.take(2).collect::<Vec<_>>())
        .await
        .expect("the server's retry replaced the 30 s delay");
    assert_eq!(items.len(), 2);
}

#[tokio::test]
async fn a_quiet_stream_closed_again_and_again_keeps_reconnecting() {
    let mock = ContractMock::start().await;
    // The server closes every stream about every ten minutes; a quiet inbox
    // gets nothing but keepalive comments in between.
    for _ in 0..8 {
        mock.respond("GET", "/inboxes/events", sse(": keepalive\n\n"));
    }
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("1", "inbox.email.received")),
    );
    let mut stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .max_reconnects(2)
        .reconnect_delay(Duration::from_millis(5))
        .await
        .unwrap();
    let item = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("the stream keeps reconnecting");
    assert!(
        matches!(item, Some(Ok(InboxStreamItem::Event(ref e))) if e.id == "1"),
        "{item:?}"
    );
    assert_eq!(
        mock.calls().len(),
        9,
        "eight quiet connections, then the event"
    );
    mock.assert_clean();
}

#[tokio::test]
async fn failed_reconnects_count_until_one_opens() {
    let mock = ContractMock::start().await;
    mock.respond("GET", "/inboxes/events", sse(": keepalive\n\n"));
    // A failure, an open quiet connection that resets the count, another
    // failure: three ends and two failures, but never two failures in a row.
    mock.error(
        "GET",
        "/inboxes/events",
        503,
        "internal_server_error",
        "Down",
    );
    mock.respond("GET", "/inboxes/events", sse(": keepalive\n\n"));
    mock.error(
        "GET",
        "/inboxes/events",
        503,
        "internal_server_error",
        "Down",
    );
    mock.respond(
        "GET",
        "/inboxes/events",
        sse(&event("7", "inbox.email.received")),
    );
    let mut stream = mock
        .client()
        .inboxes()
        .events()
        .stream()
        .max_reconnects(2)
        .reconnect_delay(Duration::from_millis(5))
        .await
        .unwrap();
    let item = stream.next().await;
    assert!(
        matches!(item, Some(Ok(InboxStreamItem::Event(ref e))) if e.id == "7"),
        "{item:?}"
    );
    assert_eq!(mock.calls().len(), 5);
}
