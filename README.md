# BillionSend Rust SDK

The official Rust client for the [BillionSend](https://billionsend.com) email API: send email, run inboxes for people and agents, stream inbox events, verify webhooks, and manage domains, contacts, broadcasts and automations.

- Every API operation (193), one method each, named after the operation in the BillionSend OpenAPI document.
- Answers as `serde_json::Value`, or any type you deserialise them into with `.json::<T>()`. Request bodies are anything that implements `serde::Serialize`.
- Errors as one enum, a variant per kind, each with the status, the API's error name, the message and the request id.
- Safe sends: every send carries an `Idempotency-Key`, so retries after a timeout never send twice.
- Streams for every cursor list and for live inbox events, with automatic resume.
- Async, on `tokio` and `reqwest` (rustls by default). Rust 1.85 or later.

Documentation: [billionsend.com/docs](https://billionsend.com/docs) · SDK guide: [billionsend.com/docs/sdks/rust](https://billionsend.com/docs/sdks/rust) · Crate docs: [docs.rs/billionsend](https://docs.rs/billionsend) · Source: [github.com/billionsend/billionsend-rust](https://github.com/billionsend/billionsend-rust)

## Install

```bash
cargo add billionsend
cargo add tokio --features macros,rt-multi-thread
cargo add serde_json
cargo add futures-util  # for the streams: lists and live events
```

TLS goes through rustls. To use the platform's TLS library instead: `cargo add billionsend --no-default-features --features native-tls`.

## Authentication

Create an API key on the dashboard (it starts with `bs_`) and pass it to the client, or set `BILLIONSEND_API_KEY`:

```rust,no_run
use billionsend::BillionSend;

let bs = BillionSend::new("bs_...");
// or, with BILLIONSEND_API_KEY (and optionally BILLIONSEND_BASE_URL) set:
let bs = BillionSend::from_env();
```

A key restricted to one inbox is what an agent should hold; see [API keys](https://billionsend.com/docs/api-keys).

## Send an email

```rust,no_run
use billionsend::BillionSend;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), billionsend::Error> {
    let bs = BillionSend::from_env();
    let email = bs
        .emails()
        .send(&json!({
            "from": "Acme <hello@acme.dev>",
            "to": ["ana@example.com"],
            "subject": "Welcome to Acme",
            "html": "<p>Glad you are here.</p>",
        }))
        .await?;
    println!("queued {}", email["id"]);
    Ok(())
}
```

Every method returns a request builder: set its options, then `.await` it. Query parameters are setters named after the parameter (`.limit(50)`, `.after(id)`); path parameters and the request body are the method's arguments. Request and response fields keep the API's snake_case names.

**Typed answers.** `.await` gives a `serde_json::Value`. To read an answer into your own type, end with `.json::<T>()`; `.raw()` gives the status and headers too:

```rust,no_run
# async fn run(bs: billionsend::BillionSend) -> Result<(), billionsend::Error> {
#[derive(serde::Deserialize)]
struct Email {
    id: String,
    subject: String,
}

let email: Email = bs.emails().get("4ef9a417-…").json().await?;
let raw = bs.domains().list().raw().await?;
println!("{} {:?} {}", raw.status, raw.request_id(), raw.body);
# Ok(()) }
```

**Idempotency.** The sending operations (`emails().send`, `emails().batch`, `emails().receiving().reply` / `forward`, `inboxes().send`, `inboxes().drafts().send`, `inboxes().threads().emails().reply` / `forward`) and `events().send` always send an `Idempotency-Key` header. Without your own, each call gets a new random key (a UUID) and that call's retries reuse it, so a retry after a timeout cannot send twice. Pass your own key to make a send safe across calls and processes too; a repeat with the same key and payload returns the first result:

```rust,no_run
# async fn run(bs: billionsend::BillionSend, message: serde_json::Value, user_id: u64) -> Result<(), billionsend::Error> {
bs.emails().send(&message).idempotency_key(format!("welcome/{user_id}")).await?;
# Ok(()) }
```

**Retries.** Failures worth retrying are retried up to `max_retries` times (default 2): `429`, `5xx`, timeouts and network errors, and the `409 concurrent_idempotent_requests` the API answers while the first attempt with the same key is still running. A `429` is retried for every request, since the API answers it before it writes anything. The other failures are retried only for requests safe to repeat: `GET`, `PUT`, `DELETE` and the operations with an `Idempotency-Key` above. An upload is not one of them, although it is a `PUT` or a `POST`. The client waits as `retry-after` asks, or backs off exponentially (0.5 s, 1 s, 2 s … up to 8 s, less up to a quarter of jitter); a `retry-after` over a minute is not waited for, and the error comes back instead.

```rust,no_run
# async fn run(message: serde_json::Value) -> Result<(), billionsend::Error> {
let bs = billionsend::BillionSend::builder().api_key("bs_...").max_retries(4).build()?;
bs.emails().send(&message).max_retries(0).await?; // per call
# Ok(()) }
```

## Errors

Every call returns `Result<T, billionsend::Error>`. The variant says what kind of failure it was:

| Variant | When |
| --- | --- |
| `Error::Validation` | `400`, `422`: the request is invalid |
| `Error::Authentication` | `401`, or no API key configured (`name()` is `"missing_api_key"`, no request made) |
| `Error::PermissionDenied` | `403`: the key, its scope, your role or your plan does not allow it |
| `Error::NotFound` | `404` |
| `Error::Conflict` | `409`, such as an idempotency key reused with a different payload |
| `Error::RateLimit` | `429`; `retry_after()` holds the server's hint in seconds |
| `Error::Server` | `5xx`; `retry_after()` holds the server's hint when it sent one |
| `Error::Api` | any other status, or a success whose body is not the expected JSON |
| `Error::Connection` | no answer: DNS, connection or TLS failure (no status) |
| `Error::Timeout` | the request timed out |
| `Error::WebhookVerification` | a webhook that does not verify |
| `Error::Application` | made up by the client without a call: an empty (or `.`, `..`) path parameter, or a header value HTTP cannot carry (`name()` is `"invalid_header"`; the message never holds the value) |

Each carries `status_code()`, `name()` (the API's error name, such as `validation_error`; `application_error` when the API sent none), `message()` and `request_id()`, the answer's `x-request-id`: the same id the request has on the dashboard's Logs page, so quote it when you ask about a request.

```rust,no_run
# async fn run(bs: billionsend::BillionSend, id: &str) -> Result<(), billionsend::Error> {
use billionsend::Error;

match bs.emails().get(id).await {
    Ok(email) => println!("{}", email["last_event"]),
    Err(Error::NotFound(_)) => println!("no such email"),
    Err(error @ Error::RateLimit(_)) => println!("retry in {:?} s", error.retry_after()),
    Err(error) => return Err(error),
}
# Ok(()) }
```

A success with no body answers `null`.

## Pagination

List methods take the API's cursor parameters (`limit`, `after`, `before`) and return one page. Every cursor list can also be turned into a stream of its items, fetching pages as you go:

```rust,no_run
# async fn run(bs: billionsend::BillionSend) -> Result<(), billionsend::Error> {
use futures_util::TryStreamExt;

let mut contacts = bs.contacts().list().limit(100).into_stream();
while let Some(contact) = contacts.try_next().await? {
    println!("{}", contact["email"]);
}
# Ok(()) }
```

Thread search pages the same way (`inboxes().threads().search(inbox).query("invoice").into_stream()`). Starting with `before` walks backward. A failed page ends the stream with its error; so does a page without a `data` array (`application_error`, "The list page has no data array"), and a page that says there is more but whose last item (first, walking backward) has no `id` to continue from, after its items.

## An agent's inbox: wait, read, reply

An agent holds a key restricted to its inbox, waits for new mail on the live event stream, reads it, and replies in the thread:

```rust,no_run
use billionsend::{BillionSend, InboxStreamItem};
use futures_util::StreamExt;
use serde_json::json;

# fn draft_answer(subject: &str, text: &str) -> String { String::new() }
#[tokio::main]
async fn main() -> Result<(), billionsend::Error> {
    let bs = BillionSend::from_env();
    let inbox = "support@acme.dev"; // an inbox id or its address

    let mut events = bs
        .inboxes()
        .events()
        .stream_inbox(inbox)
        .types("inbox.email.received")
        .await?;

    while let Some(item) = events.next().await {
        let event = match item? {
            InboxStreamItem::Event(event) => event,
            // Resumed from too far back: events may be missing. Re-read the threads.
            InboxStreamItem::Gap(_) => continue,
        };
        let ids = &event.data["data"];
        let (Some(thread_id), Some(email_id)) = (ids["thread_id"].as_str(), ids["email_id"].as_str()) else {
            continue;
        };

        let email = bs.inboxes().threads().emails().get(inbox, thread_id, email_id).await?;
        let answer = draft_answer( // your model
            email["subject"].as_str().unwrap_or_default(),
            email["text"].as_str().unwrap_or_default(),
        );
        bs.inboxes()
            .threads()
            .emails()
            .reply(inbox, thread_id, email_id, &json!({ "text": answer }))
            .idempotency_key(format!("reply/{email_id}")) // a replayed event cannot reply twice
            .await?;
    }
    Ok(())
}
```

Waiting for a sign-in code sent to the inbox:

```rust,no_run
# async fn run(bs: billionsend::BillionSend) -> Result<(), billionsend::Error> {
let code = bs.inboxes().codes().latest("support@acme.dev").from("example.com").wait(30).await?;
println!("{}", code["code"]);
# Ok(()) }
```

## Live events

`bs.inboxes().events().stream()` streams every inbox event of the team (it needs a team-wide key); `bs.inboxes().events().stream_inbox(inbox)` streams one inbox. Awaiting either opens the stream (or returns the error that refused it) and gives an `EventStream`, a `futures` `Stream` of `Result<InboxStreamItem>`:

- `InboxStreamItem::Event(InboxStreamEvent { id, event, data })`, where `event` is the webhook event name (`inbox.email.received`, `inbox.thread.created`, `inbox.draft.sent`, `email.received`, …) and `data` is the JSON `{ type, created_at, inbox_id, data }`, the webhook payload's metadata with the ids to read next (`thread_id`, `email_id`, `draft_id`).
- `InboxStreamItem::Gap(StreamGapEvent { reason })` (`stream.gap`) when the stream resumed from further back than the server keeps (24 hours): events may be missing, so read the lists again. One stream hands it out once per position.

When the connection drops (the server also closes every stream about every ten minutes), the stream reconnects with `Last-Event-ID` and replays what it missed before the live events. A reconnect the API refuses with a `retry-after` (a `429` for the stream cap, a `5xx`) waits as it asks; one asking for more than a minute ends the stream with that error. It gives up after `max_reconnects` reconnects in a row that fail to open (default 5), with the API's last refusal (`Error::RateLimit`, …) or, when the API never answered, an `Error::Connection`; a connection that opens resets the count, so a quiet inbox keeps streaming; a refusal a retry cannot fix (a revoked key) comes back as its own variant and ends the stream. One inbox's stream ends after that inbox's `inbox.deleted` or `inbox.expired`. Dropping the stream closes the connection.

```rust,no_run
# async fn run(bs: billionsend::BillionSend, saved: String) -> Result<(), billionsend::Error> {
use futures_util::StreamExt;

let mut events = bs
    .inboxes()
    .events()
    .stream()
    .types("inbox.email.received,inbox.thread.created")
    .last_event_id(saved) // resume where a previous run stopped
    .await?;
while let Some(item) = events.next().await {
    let item = item?;
    let saved = events.last_event_id(); // keep it to resume later
    # let _ = (item, saved);
}
# Ok(()) }
```

The server writes a keep-alive every 15 seconds; a connection that delivers nothing for 45 seconds is dropped and reopened (`.idle_timeout(..)` sets another limit, zero waits forever). `events.last_event_id()` is the stream's position: the last event's id, or the one the server opened the stream at.

## Verify webhooks

Every webhook delivery is signed with the endpoint's `whsec_` secret (shown when you create the endpoint). `verify_webhook` checks the signature and the timestamp (within 5 minutes by default, so a captured delivery cannot be replayed) and returns the parsed event; anything wrong is an `Error::WebhookVerification`. Pass the body exactly as it arrived, before any JSON parsing. The headers can be a `reqwest`/`http` `HeaderMap`, a `HashMap` or a list of pairs:

```rust,no_run
# fn handle(raw_body: &[u8], headers: &reqwest::header::HeaderMap) -> Result<(), billionsend::Error> {
let secret = std::env::var("BILLIONSEND_WEBHOOK_SECRET").expect("secret");
let event = billionsend::verify_webhook(raw_body, headers, &secret)?;
if event["type"] == "email.bounced" {
    // ...
}
# Ok(()) }
```

`WebhookVerifier::new(secret)?.tolerance(..)` sets another tolerance. During a secret rotation (`webhooks().rotate_signing_secret`) a delivery carries a signature for each live secret, and either secret verifies it.

## Uploads

The two logo uploads and the contact import send `multipart/form-data`. The file is anything that converts into a `FileUpload`: bytes (`Vec<u8>`, `&[u8]`, `bytes::Bytes`), or a `FileUpload` with its own `.file_name(..)` and `.content_type(..)`. A logo is a PNG, JPEG or WebP of at most 2 MiB, named and typed from its bytes unless you say otherwise; a CSV is sent as `contacts.csv`. An import's fields are setters, and `column_map`, `segments` and `topics` take anything `serde::Serialize` (a string is sent as it is, already JSON):

```rust,no_run
# async fn run(bs: billionsend::BillionSend, domain_id: &str) -> Result<(), Box<dyn std::error::Error>> {
use serde_json::json;

bs.domains().unsubscribe_page().logo().upload(domain_id, std::fs::read("logo.png")?).await?;

let import = bs
    .contacts()
    .imports()
    .create(std::fs::read("people.csv")?)
    .column_map(&json!({ "email": "Email", "first_name": "First Name" }))
    .on_conflict("skip")
    .await?;
println!("{}", import["id"]);
# Ok(()) }
```

An upload is not repeated after a `5xx`, a timeout or a dropped connection; a `429` is retried.

## Agent sign-up

An agent can create its own account with no API key; the answer carries the new key and inbox.

```rust,no_run
# async fn run() -> Result<(), billionsend::Error> {
use billionsend::BillionSend;
use serde_json::json;

let signup = BillionSend::from_env()
    .agent()
    .signup(&json!({ "human_email": "owner@acme.dev", "name": "Support agent" }))
    .await?;
let agent = BillionSend::new(signup["api_key"].as_str().unwrap_or_default());
println!("{}", signup["inbox"]["email_address"]);
# let _ = agent;
# Ok(()) }
```

## Configuration

```rust,no_run
# fn run() -> Result<(), billionsend::Error> {
use std::time::Duration;

let bs = billionsend::BillionSend::builder()
    .api_key("bs_...")                       // default: BILLIONSEND_API_KEY
    .base_url("https://api.billionsend.com") // default: BILLIONSEND_BASE_URL, then this
    .timeout(Duration::from_secs(60))        // per attempt; event streams are timed only until they open
    .max_retries(2)                          // see Retries
    .header("x-my-header", "value")          // sent with every request
    .http_client(reqwest::Client::new())     // your own reqwest client (proxies, TLS roots)
    .build()?;
# Ok(()) }
```

Every request builder also has `.header(..)`, `.timeout(..)` and `.max_retries(..)` for that call, and `.idempotency_key(..)` on the sending operations.

**Batches.** `emails().batch` and `contacts().batch().create` take `.batch_validation("strict" | "permissive")` (the `x-batch-validation` header).

## Resources

| Method | Covers |
| --- | --- |
| `emails()` | send, batch, list, get, update, cancel, delete, insights, share, unshare, `attachments()` (list, get) |
| `emails().metrics()` | `get` (the counts over time), and `links`, `countries`, `clients` |
| `emails().receiving()` | received mail: list, get, delete, reply, forward, `attachments()` (list, get, text) |
| `domains()` | create, list, get, update, delete, verify, `claims()` (create, get, verify), `unsubscribe_page().logo()` (upload, delete) |
| `api_keys()` | create, list, update, delete |
| `contacts()` | create, list, get, update, delete, preferences_link, `batch()`, `topics()`, `segments()` (list, add, remove), `imports()` (create, list, get) |
| `audiences().contacts()` | contacts addressed through an audience |
| `contact_properties()`, `segments()`, `topics()` | create, list, get, update, delete (segment contacts under `segments().contacts()`) |
| `broadcasts()` | create, list, get, update, delete, send, duplicate, cancel, recipients, clicked_links |
| `broadcast_groups()` | create, list, get, update, remove, send, cancel (one send from several domains) |
| `automations()` | create, list, get, update, remove, validate, duplicate, simulate, stop (takes an optional body, `cancel_runs`), metrics, `runs()` (list, get, cancel) |
| `events()` | create, list, get, update, remove, send (custom events) |
| `templates()` | create, list, get, update, delete, publish, duplicate |
| `webhooks()` | create, list, get, update, delete, rotate_signing_secret, `events()` (list, get, replay, `attempts()`) |
| `suppressions()` | create, list, get, delete, `batch()` add / remove |
| `unsubscribes()` | create, list |
| `verifications()` | availability, estimate, create, list, get |
| `usage()`, `deliverability()` | get |
| `brand()` | get, update, `logo()` (upload, delete) |
| `logs()` | list, get |
| `oauth_grants()` | list, revoke (connected apps) |
| `inboxes()` | create, list, get, update, delete, send, `agent()` (get, update), `safety()`, `labels()`, `threads()` (search, labels, emails, reply, forward), `quarantine()`, `codes()`, `drafts()`, `events()` |
| `agent()` | signup, status, claim_email |

Each method is the operation's `operationId` in snake_case: `inboxes.threads.labels.set` is `bs.inboxes().threads().labels().set(..)`. One operation is also the group of others: `emails.metrics` is `bs.emails().metrics().get()`, beside `emails.metrics.links` / `countries` / `clients` as `bs.emails().metrics().links()` and so on.

## Development

The library builds on Rust 1.85 or later (CI checks it with 1.85); the tests and the `xtask` need a recent stable toolchain, since their dev-dependencies require a newer Rust.

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo xtask generate --check
```

The contract is `openapi/api.openapi.json`, a snapshot of the API's OpenAPI document. `src/generated.rs` (the resources, their methods and one request builder per operation) is generated from it, its descriptions becoming the doc comments. The tests run every SDK method against a local mock that validates each request and answers from that snapshot, and fail when an operation has no SDK method or a request leaves the contract. When the API changes:

```bash
cargo xtask sync --url https://billionsend.com/docs/api.openapi.json   # or: --from path/to/api.openapi.json
cargo xtask generate           # regenerates src/generated.rs
cargo test                     # what fails is what the SDK has to follow
```

## Releasing

Publishing is manual; nothing is published on push.

1. Set `version` in `Cargo.toml`, turn the changelog's `Unreleased` heading into that version, commit, then tag and push: `git tag -a v0.3.0 -m "release 0.3.0" && git push origin main v0.3.0`.
2. The first version only: an owner of the crate's crates.io account publishes from a clean checkout of the tag with an API token (scope `publish-new`): `cargo test && cargo publish --dry-run && cargo publish`.
3. Once the crate exists: on crates.io, `billionsend` → Settings → Trusted Publishing → GitHub, repository `billionsend/billionsend-rust`, workflow `release.yml`, environment `crates-io`.
4. Every later version: Actions → `release` → Run workflow, on the version tag. The workflow refuses a branch or a tag that differs from `Cargo.toml`, runs every check, and publishes with crates.io Trusted Publishing (no token stored).

## License

[MIT](https://github.com/billionsend/billionsend-rust/blob/main/LICENSE)
