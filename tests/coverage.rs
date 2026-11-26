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
use common::{CSV, ContractMock, ID, ID2, INBOX, OPERATIONS, PNG, body_example, operation_keys};
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
        ("PATCH", "/api-keys/{id}") => c.api_keys().update(ID, &b),
        ("POST", "/audiences/{audienceId}/contacts") => c.audiences().contacts().create(ID, &b),
        ("DELETE", "/audiences/{audienceId}/contacts/{id}") => c.audiences().contacts().delete(ID, ID2),
        ("GET", "/audiences/{audienceId}/contacts/{id}") => c.audiences().contacts().get(ID, ID2),
        ("PATCH", "/audiences/{audienceId}/contacts/{id}") => c.audiences().contacts().update(ID, ID2, &b),
        ("POST", "/automations") => c.automations().create(&b),
        ("POST", "/automations/{id}/duplicate") => c.automations().duplicate(ID),
        ("GET", "/automations/{id}") => c.automations().get(ID),
        ("GET", "/automations") => c.automations().list().status("enabled"),
        ("GET", "/automations/{id}/metrics") => c.automations().metrics(ID),
        ("DELETE", "/automations/{id}") => c.automations().remove(ID),
        ("POST", "/automations/{id}/runs/{run_id}/cancel") => c.automations().runs().cancel(ID, ID2),
        ("GET", "/automations/{id}/runs/{run_id}") => c.automations().runs().get(ID, ID2),
        ("GET", "/automations/{id}/runs") => c.automations().runs().list(ID),
        ("POST", "/automations/{id}/simulate") => c.automations().simulate(ID, &b),
        ("POST", "/automations/{id}/stop") => c.automations().stop(ID),
        ("PATCH", "/automations/{id}") => c.automations().update(ID, &b),
        ("POST", "/automations/validate") => c.automations().validate(&b),
        ("GET", "/brand") => c.brand().get(),
        ("DELETE", "/brand/logo") => c.brand().logo().delete(),
        ("PUT", "/brand/logo") => c.brand().logo().upload(PNG),
        ("PATCH", "/brand") => c.brand().update(&b),
        ("POST", "/broadcasts/{id}/cancel") => c.broadcasts().cancel(ID),
        ("GET", "/broadcasts/{id}/clicked-links") => c.broadcasts().clicked_links(ID),
        ("POST", "/broadcasts") => c.broadcasts().create(&b),
        ("DELETE", "/broadcasts/{id}") => c.broadcasts().delete(ID),
        ("POST", "/broadcasts/{id}/duplicate") => c.broadcasts().duplicate(ID),
        ("GET", "/broadcasts/{id}") => c.broadcasts().get(ID),
        ("GET", "/broadcasts") => c.broadcasts().list(),
        ("GET", "/broadcasts/{id}/recipients") => c.broadcasts().recipients(ID).r#type("delivered"),
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
        ("POST", "/contacts/imports") => c.contacts().imports().create(CSV).on_conflict("skip"),
        ("GET", "/contacts/imports/{id}") => c.contacts().imports().get(ID),
        ("GET", "/contacts/imports") => c.contacts().imports().list(),
        ("GET", "/contacts/{id}/segments") => c.contacts().segments().list(ID),
        ("POST", "/contacts/{id}/preferences-link") => c.contacts().preferences_link(ID),
        ("POST", "/contacts/{id}/segments/{segmentId}") => c.contacts().segments().add(ID, ID2),
        ("DELETE", "/contacts/{id}/segments/{segmentId}") => c.contacts().segments().remove(ID, ID2),
        ("GET", "/contacts/{id}/topics") => c.contacts().topics().list(ID),
        ("PATCH", "/contacts/{id}/topics") => c.contacts().topics().update(ID, &b),
        ("PATCH", "/contacts/{id}") => c.contacts().update(ID, &b),
        ("GET", "/deliverability") => c.deliverability().get(),
        ("POST", "/domains") => c.domains().create(&b),
        ("POST", "/domains/claim") => c.domains().claims().create(&b),
        ("GET", "/domains/{id}/claim") => c.domains().claims().get(ID),
        ("POST", "/domains/{id}/claim/verify") => c.domains().claims().verify(ID),
        ("DELETE", "/domains/{id}/unsubscribe-page/logo") => c.domains().unsubscribe_page().logo().delete(ID),
        ("PUT", "/domains/{id}/unsubscribe-page/logo") => c.domains().unsubscribe_page().logo().upload(ID, PNG),
        ("DELETE", "/domains/{id}") => c.domains().delete(ID),
        ("GET", "/domains/{id}") => c.domains().get(ID),
        ("GET", "/domains") => c.domains().list(),
        ("PATCH", "/domains/{id}") => c.domains().update(ID, &b),
        ("POST", "/domains/{id}/verify") => c.domains().verify(ID),
        ("POST", "/emails/batch") => c.emails().batch(&b).idempotency_key("key-2").batch_validation("permissive"),
        ("GET", "/emails/{email_id}/attachments/{id}") => c.emails().attachments().get(ID, ID2),
        ("GET", "/emails/{email_id}/attachments") => c.emails().attachments().list(ID),
        ("POST", "/emails/{id}/share") => c.emails().share(ID, &b),
        ("DELETE", "/emails/{id}/share") => c.emails().unshare(ID),
        ("POST", "/events") => c.events().create(&b),
        ("GET", "/events/{id_or_name}") => c.events().get(ID),
        ("GET", "/events") => c.events().list(),
        ("DELETE", "/events/{id_or_name}") => c.events().remove(ID),
        ("POST", "/events/send") => c.events().send(&b).idempotency_key("key-4"),
        ("PATCH", "/events/{id_or_name}") => c.events().update(ID, &b),
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
        ("PATCH", "/inboxes/{inbox_id}/agent") => c.inboxes().agent().update(INBOX, &b),
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
        ("GET", "/logs/{id}") => c.logs().get(ID),
        ("GET", "/logs") => c.logs().list(),
        ("GET", "/oauth/grants") => c.oauth_grants().list(),
        ("DELETE", "/oauth/grants/{id}") => c.oauth_grants().revoke(ID),
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
        ("GET", "/webhooks/{id}/events/{event_id}/attempts") => c.webhooks().events().attempts().list(ID, ID2),
        ("GET", "/webhooks/{id}/events/{event_id}") => c.webhooks().events().get(ID, ID2),
        ("GET", "/webhooks/{id}/events") => c.webhooks().events().list(ID),
        ("POST", "/webhooks/{id}/events/{event_id}/replay") => c.webhooks().events().replay(ID, ID2),
        ("POST", "/webhooks/{id}/signing-secret/rotate") => c.webhooks().rotate_signing_secret(ID, &b),
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
    assert_eq!(table.len(), 186);
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

/// Each method is its operation's `operationId` in snake_case: the table above
/// is written by hand, so its calls are held to the document's names.
#[test]
fn every_method_is_named_after_its_operation_id() {
    let source = include_str!("coverage.rs");
    for op in OPERATIONS.iter() {
        let id = op.spec()["operationId"].as_str().unwrap();
        let row = source
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with(&format!("(\"{}\", \"{}\") =>", op.method, op.path)))
            .unwrap_or_else(|| panic!("{id} has no row"));
        let mut segments: Vec<String> = id.split('.').map(snake).collect();
        // An operation that is also the group of others is the group's `get`.
        let group = format!("{id}.");
        if OPERATIONS.iter().any(|other| {
            other.spec()["operationId"]
                .as_str()
                .is_some_and(|o| o.starts_with(&group))
        }) {
            segments.push("get".to_owned());
        }
        let last = segments.pop().unwrap();
        let groups: String = segments.iter().map(|s| format!("{s}().")).collect();
        let call = format!("c.{groups}{last}(");
        assert!(row.contains(&call), "{id}: expected {call} in {row}");
    }
}

fn snake(segment: &str) -> String {
    let mut out = String::new();
    for c in segment.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}
