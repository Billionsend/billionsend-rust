//! Every operation of the snapshot is reachable through the SDK, and nothing else is.
//!
//! `calls()` maps each (method, path) of the contract to the SDK call that
//! serves it. The test runs every call through the contract mock, once with
//! answers that carry every property and once with only the required ones, and
//! compares the operations hit with the snapshot: an operation the SDK does not
//! reach, or a request outside the contract, fails.

mod common;

use std::collections::BTreeSet;
use std::future::{Future, IntoFuture};
use std::pin::Pin;

use billionsend::{BillionSend, InboxStreamItem, Result};
use common::{ContractMock, ID, ID2, INBOX, OPERATIONS, body_example, operation_keys};
use futures_util::StreamExt;
use serde_json::{Value, json};

type Call = Box<dyn Fn(BillionSend, Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>>>;

/// Opens a stream, reads its first event and closes it.
async fn first_event<R>(request: R) -> Result<Value>
where
    R: IntoFuture<Output = Result<billionsend::EventStream>>,
{
    let mut stream = request.await?;
    match stream.next().await {
        Some(Ok(InboxStreamItem::Event(event))) => {
            Ok(json!({ "id": event.id, "event": event.event }))
        }
        other => panic!("expected an event, got {other:?}"),
    }
}

macro_rules! calls {
    ($c:ident, $b:ident; $(($method:literal, $path:literal) => $call:expr,)*) => {
        vec![$(
            (
                $method,
                $path,
                Box::new(|$c: BillionSend, $b: Value| {
                    Box::pin(async move {
                        let _ = &$b;
                        $call.await
                    }) as Pin<Box<dyn Future<Output = Result<Value>> + Send>>
                }) as Call,
            ),
        )*]
    };
}

fn calls() -> Vec<(&'static str, &'static str, Call)> {
    calls! { c, b;
        ("POST", "/agent/claim-email") => c.agent().claim_email(),
        ("POST", "/agent/signup") => c.agent().signup(&b),
        ("GET", "/agent/status") => c.agent().status(),
        ("POST", "/api-keys") => c.api_keys().create(&b),
        ("DELETE", "/api-keys/{id}") => c.api_keys().delete(ID),
        ("GET", "/api-keys") => c.api_keys().list(),
        ("POST", "/audiences/{audienceId}/contacts") => c.audiences().contacts().create(ID, &b),
        ("DELETE", "/audiences/{audienceId}/contacts/{id}") => c.audiences().contacts().delete(ID, ID2),
        ("GET", "/audiences/{audienceId}/contacts/{id}") => c.audiences().contacts().get(ID, ID2),
        ("PATCH", "/audiences/{audienceId}/contacts/{id}") => c.audiences().contacts().update(ID, ID2, &b),
        ("POST", "/broadcasts/{id}/cancel") => c.broadcasts().cancel(ID),
        ("POST", "/broadcasts") => c.broadcasts().create(&b),
        ("DELETE", "/broadcasts/{id}") => c.broadcasts().delete(ID),
        ("POST", "/broadcasts/{id}/duplicate") => c.broadcasts().duplicate(ID),
        ("GET", "/broadcasts/{id}") => c.broadcasts().get(ID),
        ("GET", "/broadcasts") => c.broadcasts().list(),
        ("POST", "/broadcasts/{id}/send") => c.broadcasts().send(ID, &b),
        ("PATCH", "/broadcasts/{id}") => c.broadcasts().update(ID, &b),
        ("POST", "/contact-properties") => c.contact_properties().create(&b),
        ("DELETE", "/contact-properties/{id}") => c.contact_properties().delete(ID),
        ("GET", "/contact-properties/{id}") => c.contact_properties().get(ID),
        ("GET", "/contact-properties") => c.contact_properties().list(),
        ("PATCH", "/contact-properties/{id}") => c.contact_properties().update(ID, &b),
        ("POST", "/contacts/batch") => c.contacts().batch().create(&b).on_conflict("upsert").batch_validation("permissive"),
        ("POST", "/contacts/batch/get") => c.contacts().batch().get(&b),
        ("POST", "/contacts/batch/remove") => c.contacts().batch().remove(&b),
        ("POST", "/contacts") => c.contacts().create(&b),
        ("DELETE", "/contacts/{id}") => c.contacts().delete(ID).erase("true"),
        ("GET", "/contacts/{id}") => c.contacts().get(ID),
        ("GET", "/contacts") => c.contacts().list(),
        ("POST", "/contacts/{id}/preferences-link") => c.contacts().preferences_link(ID),
        ("POST", "/contacts/{id}/segments/{segmentId}") => c.contacts().segments().add(ID, ID2),
        ("DELETE", "/contacts/{id}/segments/{segmentId}") => c.contacts().segments().remove(ID, ID2),
        ("GET", "/contacts/{id}/topics") => c.contacts().topics().list(ID),
        ("PATCH", "/contacts/{id}/topics") => c.contacts().topics().update(ID, &b),
        ("PATCH", "/contacts/{id}") => c.contacts().update(ID, &b),
        ("GET", "/deliverability") => c.deliverability().get(),
        ("POST", "/domains") => c.domains().create(&b),
        ("DELETE", "/domains/{id}") => c.domains().delete(ID),
        ("GET", "/domains/{id}") => c.domains().get(ID),
        ("GET", "/domains") => c.domains().list(),
        ("PATCH", "/domains/{id}") => c.domains().update(ID, &b),
        ("POST", "/domains/{id}/verify") => c.domains().verify(ID),
        ("POST", "/emails/batch") => c.emails().batch(&b).idempotency_key("key-2").batch_validation("permissive"),
        ("POST", "/emails/{id}/cancel") => c.emails().cancel(ID),
        ("DELETE", "/emails/{id}") => c.emails().delete(ID),
        ("GET", "/emails/{id}") => c.emails().get(ID),
        ("GET", "/emails/{id}/insights") => c.emails().insights(ID),
        ("GET", "/emails") => c.emails().list().limit(10),
        ("GET", "/emails/metrics") => c.emails().metrics().get().metrics(["sent", "delivered"]).dimensions(["period"]).granularity("daily"),
        ("GET", "/emails/metrics/clients") => c.emails().metrics().clients(),
        ("GET", "/emails/metrics/countries") => c.emails().metrics().countries(),
        ("GET", "/emails/metrics/links") => c.emails().metrics().links().domain_id([ID, ID2]).limit(5),
        ("GET", "/emails/receiving/{email_id}/attachments/{id}") => c.emails().receiving().attachments().get(ID, ID2),
        ("GET", "/emails/receiving/{email_id}/attachments") => c.emails().receiving().attachments().list(ID),
        ("GET", "/emails/receiving/{email_id}/attachments/{id}/text") => c.emails().receiving().attachments().text(ID, ID2),
        ("DELETE", "/emails/receiving/{id}") => c.emails().receiving().delete(ID),
        ("POST", "/emails/receiving/{id}/forward") => c.emails().receiving().forward(ID, &b),
        ("GET", "/emails/receiving/{id}") => c.emails().receiving().get(ID).html_format("cid"),
        ("GET", "/emails/receiving") => c.emails().receiving().list().unread("true").from("billing@example.com"),
        ("POST", "/emails/receiving/{id}/reply") => c.emails().receiving().reply(ID, &b),
        ("POST", "/emails") => c.emails().send(&b).idempotency_key("key-1"),
        ("PATCH", "/emails/{id}") => c.emails().update(ID, &b),
        ("GET", "/inboxes/{inbox_id}/agent") => c.inboxes().agent().get(INBOX),
        ("GET", "/inboxes/{inbox_id}/codes/latest") => c.inboxes().codes().latest(INBOX).wait(5).max_age(600),
        ("POST", "/inboxes") => c.inboxes().create(&b),
        ("DELETE", "/inboxes/{inbox_id}") => c.inboxes().delete(INBOX),
        ("POST", "/inboxes/{inbox_id}/drafts") => c.inboxes().drafts().create(INBOX, &b),
        ("DELETE", "/inboxes/{inbox_id}/drafts/{draft_id}") => c.inboxes().drafts().delete(INBOX, ID),
        ("GET", "/inboxes/{inbox_id}/drafts/{draft_id}") => c.inboxes().drafts().get(INBOX, ID),
        ("GET", "/inboxes/{inbox_id}/drafts") => c.inboxes().drafts().list(INBOX),
        ("POST", "/inboxes/{inbox_id}/drafts/{draft_id}/reject") => c.inboxes().drafts().reject(INBOX, ID),
        ("POST", "/inboxes/{inbox_id}/drafts/{draft_id}/send") => c.inboxes().drafts().send(INBOX, ID, &b),
        ("PATCH", "/inboxes/{inbox_id}/drafts/{draft_id}") => c.inboxes().drafts().update(INBOX, ID, &b),
        ("GET", "/inboxes/events") => first_event(c.inboxes().events().stream().types("inbox.email.received").max_reconnects(0)),
        ("GET", "/inboxes/{inbox_id}/events") => first_event(c.inboxes().events().stream_inbox(INBOX).max_reconnects(0)),
        ("GET", "/inboxes/{inbox_id}") => c.inboxes().get(INBOX),
        ("POST", "/inboxes/{inbox_id}/labels") => c.inboxes().labels().create(INBOX, &b),
        ("DELETE", "/inboxes/{inbox_id}/labels/{label_id}") => c.inboxes().labels().delete(INBOX, ID),
        ("GET", "/inboxes/{inbox_id}/labels") => c.inboxes().labels().list(INBOX),
        ("PATCH", "/inboxes/{inbox_id}/labels/{label_id}") => c.inboxes().labels().update(INBOX, ID, &b),
        ("GET", "/inboxes") => c.inboxes().list().kind("agent").limit(5),
        ("GET", "/inboxes/{inbox_id}/quarantine") => c.inboxes().quarantine().list(INBOX),
        ("POST", "/inboxes/{inbox_id}/quarantine/{email_id}/release") => c.inboxes().quarantine().release(INBOX, ID, &b),
        ("GET", "/inboxes/{inbox_id}/safety") => c.inboxes().safety().get(INBOX),
        ("PATCH", "/inboxes/{inbox_id}/safety") => c.inboxes().safety().update(INBOX, &b),
        ("POST", "/inboxes/{inbox_id}/emails") => c.inboxes().send(INBOX, &b),
        ("DELETE", "/inboxes/{inbox_id}/threads/{thread_id}") => c.inboxes().threads().delete(INBOX, ID),
        ("POST", "/inboxes/{inbox_id}/threads/{thread_id}/emails/{email_id}/forward") => c.inboxes().threads().emails().forward(INBOX, ID, ID2, &b),
        ("GET", "/inboxes/{inbox_id}/threads/{thread_id}/emails/{email_id}") => c.inboxes().threads().emails().get(INBOX, ID, ID2),
        ("GET", "/inboxes/{inbox_id}/threads/{thread_id}/emails") => c.inboxes().threads().emails().list(INBOX, ID),
        ("POST", "/inboxes/{inbox_id}/threads/{thread_id}/emails/{email_id}/reply") => c.inboxes().threads().emails().reply(INBOX, ID, ID2, &b).idempotency_key("key-3"),
        ("GET", "/inboxes/{inbox_id}/threads/{thread_id}") => c.inboxes().threads().get(INBOX, ID),
        ("POST", "/inboxes/{inbox_id}/threads/{thread_id}/labels") => c.inboxes().threads().labels().add(INBOX, ID, &b),
        ("DELETE", "/inboxes/{inbox_id}/threads/{thread_id}/labels/{label_id}") => c.inboxes().threads().labels().remove(INBOX, ID, ID2),
        ("PUT", "/inboxes/{inbox_id}/threads/{thread_id}/labels") => c.inboxes().threads().labels().set(INBOX, ID, &b),
        ("GET", "/inboxes/{inbox_id}/threads") => c.inboxes().threads().list(INBOX).read(false).include_quarantined(true),
        ("GET", "/inboxes/{inbox_id}/threads/search") => c.inboxes().threads().search(INBOX).query("invoice").from("billing@example.com").has_attachment(true),
        ("PATCH", "/inboxes/{inbox_id}/threads/{thread_id}") => c.inboxes().threads().update(INBOX, ID, &b),
        ("PATCH", "/inboxes/{inbox_id}") => c.inboxes().update(INBOX, &b),
        ("GET", "/segments/{id}/contacts") => c.segments().contacts().list(ID),
        ("POST", "/segments") => c.segments().create(&b),
        ("DELETE", "/segments/{id}") => c.segments().delete(ID),
        ("GET", "/segments/{id}") => c.segments().get(ID),
        ("GET", "/segments") => c.segments().list(),
        ("PATCH", "/segments/{id}") => c.segments().update(ID, &b),
        ("POST", "/suppressions/batch/add") => c.suppressions().batch().add(&b),
        ("POST", "/suppressions/batch/remove") => c.suppressions().batch().remove(&b),
        ("POST", "/suppressions") => c.suppressions().create(&b),
        ("DELETE", "/suppressions/{id}") => c.suppressions().delete(ID),
        ("GET", "/suppressions/{id}") => c.suppressions().get(ID),
        ("GET", "/suppressions") => c.suppressions().list().origin("bounce"),
        ("POST", "/templates") => c.templates().create(&b),
        ("DELETE", "/templates/{id}") => c.templates().delete(ID),
        ("POST", "/templates/{id}/duplicate") => c.templates().duplicate(ID),
        ("GET", "/templates/{id}") => c.templates().get(ID),
        ("GET", "/templates") => c.templates().list(),
        ("POST", "/templates/{id}/publish") => c.templates().publish(ID),
        ("PATCH", "/templates/{id}") => c.templates().update(ID, &b),
        ("POST", "/topics") => c.topics().create(&b),
        ("DELETE", "/topics/{id}") => c.topics().delete(ID),
        ("GET", "/topics/{id}") => c.topics().get(ID),
        ("GET", "/topics") => c.topics().list(),
        ("PATCH", "/topics/{id}") => c.topics().update(ID, &b),
        ("POST", "/unsubscribes") => c.unsubscribes().create(&b),
        ("DELETE", "/unsubscribes/{id}") => c.unsubscribes().delete(ID),
        ("GET", "/unsubscribes") => c.unsubscribes().list().domain_id(ID),
        ("GET", "/usage") => c.usage().get(),
        ("GET", "/verifications/availability") => c.verifications().availability(),
        ("POST", "/verifications") => c.verifications().create(&b),
        ("POST", "/verifications/estimate") => c.verifications().estimate(&b),
        ("GET", "/verifications/{id}") => c.verifications().get(ID),
        ("GET", "/verifications") => c.verifications().list(),
        ("POST", "/webhooks") => c.webhooks().create(&b),
        ("DELETE", "/webhooks/{id}") => c.webhooks().delete(ID),
        ("GET", "/webhooks/{id}") => c.webhooks().get(ID),
        ("GET", "/webhooks") => c.webhooks().list(),
        ("POST", "/webhooks/{id}/rotate") => c.webhooks().rotate(ID, &b),
        ("PATCH", "/webhooks/{id}") => c.webhooks().update(ID, &b),
    }
}

#[test]
fn the_table_names_every_operation_once() {
    let table: Vec<(String, String)> = calls()
        .into_iter()
        .map(|(m, p, _)| (m.to_owned(), p.to_owned()))
        .collect();
    let unique: BTreeSet<(String, String)> = table.iter().cloned().collect();
    assert_eq!(unique.len(), table.len(), "an operation is listed twice");
    assert_eq!(unique, operation_keys());
    assert_eq!(table.len(), 139);
}

async fn run_all(minimal: bool) {
    let mock = ContractMock::start().await;
    mock.minimal(minimal);
    let client = mock.client();
    for (method, path, call) in calls() {
        let op = OPERATIONS
            .iter()
            .find(|o| o.method == method && o.path == path)
            .unwrap();
        mock.clear();
        let result = call(client.clone(), body_example(op)).await;
        if let Err(error) = &result {
            panic!("{method} {path}: {error}");
        }
        let hits = mock.hits();
        let expected: BTreeSet<(String, String)> = [(method.to_owned(), path.to_owned())].into();
        assert_eq!(
            hits, expected,
            "{method} {path} reached the wrong operation"
        );
    }
    mock.assert_clean();
}

#[tokio::test]
async fn every_operation_with_full_answers() {
    run_all(false).await;
}

#[tokio::test]
async fn every_operation_with_minimal_answers() {
    run_all(true).await;
}

#[tokio::test]
async fn query_parameters_reach_the_request() {
    let mock = ContractMock::start().await;
    let client = mock.client();
    client
        .emails()
        .metrics()
        .get()
        .metrics(["sent", "delivered"])
        .granularity("daily")
        .await
        .unwrap();
    let call = mock.last();
    assert_eq!(call.query("metrics"), Some("sent,delivered"));
    assert_eq!(call.query("granularity"), Some("daily"));

    client
        .inboxes()
        .threads()
        .search(INBOX)
        .query("invoice")
        .from("billing@example.com")
        .has_attachment(true)
        .limit(5)
        .await
        .unwrap();
    let call = mock.last();
    assert_eq!(call.path_params["inbox_id"], INBOX);
    assert_eq!(call.query("query"), Some("invoice"));
    assert_eq!(call.query("from"), Some("billing@example.com"));
    assert_eq!(call.query("has_attachment"), Some("true"));
    assert_eq!(call.query("limit"), Some("5"));
    mock.assert_clean();
}
