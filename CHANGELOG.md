# Changelog

## 0.3.0 (2026-11-28)

The OpenAPI snapshot follows the published document (193 operations, up from 186).

- New operations: multi-domain sends as `broadcast_groups()` (`create`, `list`, `get`, `update`, `remove`, `send`, `cancel`): one group names two to twenty of your verified domains and gets one broadcast per domain, each mailing the contacts that belong to it. `broadcast_groups().list()` has `into_stream()` like every cursor list.
- Changed: `automations().stop(id, &body)` takes the request body the document now declares (`cancel_runs`, to cancel the runs already going as well; pass `&json!({})` to let them finish). This changes the method's signature.
- Doc comments follow the published descriptions, which also changed for broadcasts (`domain_id`, the group they belong to), domains, emails, topics and contact topics.

## 0.2.0 (2026-11-26)

The OpenAPI snapshot follows the published document (186 operations, up from 139), and the SDK follows it.

- New operations: automations (`automations().create`, `list`, `get`, `update`, `remove`, `validate`, `duplicate`, `simulate`, `stop`, `metrics`; `automations().runs().list`, `get`, `cancel`), custom events (`events().create`, `list`, `get`, `update`, `remove`, `send`, the last with an `Idempotency-Key` like the sending operations), brand defaults (`brand().get`, `update`; `brand().logo().upload`, `delete`), contact imports (`contacts().imports().create`, `list`, `get`), `contacts().segments().list`, domain claims (`domains().claims().create`, `get`, `verify`), `domains().unsubscribe_page().logo().upload` and `delete`, `emails().share` and `unshare`, `emails().attachments().list` and `get`, `broadcasts().recipients` and `clicked_links`, `logs().list` and `get`, `oauth_grants().list` and `revoke`, `webhooks().events().list`, `get`, `replay` and `events().attempts().list`, `webhooks().rotate_signing_secret`, `api_keys().update` and `inboxes().agent().update`. Every cursor list has `into_stream()`.
- Removed: `unsubscribes().delete` (a recipient's opt-out can no longer be deleted through the API) and `webhooks().rotate` (replaced by `webhooks().rotate_signing_secret`).
- New query parameter: `suppressions().list().kind("email" | "domain" | "all")`. Suppressions can block a whole domain and carry a `note`; entries carry `kind`, `domain` and `removable`, and `email` is `null` on a domain entry. Doc comments follow the published descriptions, which also cover `domains().get`'s `sending_limits`, the unsubscribe page's new fields, a broadcast's `dormant_check` and `include_dormant`, `deliverability().get`'s `domains` and the 403 `restricted_api_key`.
- Uploads (`multipart/form-data`) take a `FileUpload`: bytes, with an optional file name and content type. An upload is not repeated after a 5xx, a timeout or a dropped connection; a 429 repeats it.
- A 429 is retried for every request, a `POST` or `PATCH` without an idempotency key included (the API answers it before it writes anything).
- A success with no body (a 204, an empty 200) answers `null` instead of an `Error::Api`.
- A path parameter of `.` or `..` is refused before any request, like an empty one.
- A header value HTTP cannot carry (non-ASCII or a control character) is an `Error::Application` named `invalid_header`.
- The API key is read without the whitespace around it, and a blank one counts as none, so the environment's is used.
- Event streams: an `id` counts once its block is complete; an id sent without data moves the position, so a connection that drops before its first event resumes from there; a connection that delivers nothing for 45 seconds (`idle_timeout`) is dropped and reopened.
- `verify_webhook` accepts a secret and headers with surrounding whitespace and a signature list separated by any whitespace.

## 0.1.2 (2026-11-17)

- An inbox event stream whose reconnect the API refuses with a `retry-after` (a 429 for the stream cap, a 5xx) waits as it asks before trying again; one asking for more than a minute ends the stream with that error at once. When the reconnects run out, the stream ends with the API's own last refusal (`Error::RateLimit`, `Error::Server`, …) instead of a general `Error::Connection`, which stays for reconnects that got no answer at all.
- `Error::retry_after()` now also holds a 5xx answer's `retry-after`, not only a 429's.

## 0.1.1 (2026-11-17)

- Inbox event streams no longer give up on a quiet inbox. The server closes every stream about every ten minutes, and a connection that opened and then ended normally used to count as a failed reconnect, so after about an hour without events the stream ended with "gave up after 5 reconnects". Now a connection that opens resets the count, and `max_reconnects` counts only reconnects in a row that fail to open.

## 0.1.0 (2026-11-17)

- First version of the BillionSend SDK for Rust: all 139 operations of the BillionSend API, grouped by resource, each method named after its `operationId` in snake_case, generated from the OpenAPI document with its descriptions as doc comments. `emails.metrics`, which is also the group of `links`, `countries` and `clients`, is `emails().metrics().get()`.
- Request builders: path parameters and the body are arguments, query parameters are setters, and `.await` sends. Answers are `serde_json::Value`; `.json::<T>()` deserialises into your own type and `.raw()` keeps the status and headers. Request bodies take anything `serde::Serialize`.
- Errors as one enum, `Error::{Validation, Authentication, PermissionDenied, NotFound, Conflict, RateLimit, Server, Api, Connection, Timeout, WebhookVerification, Application}`, each with `status_code()`, `name()`, `message()` and `request_id()` (the answer's `x-request-id`), and `retry_after()` on a 429.
- The sending operations always carry an `Idempotency-Key`: the caller's, or a new UUID per call that its retries reuse.
- Retries for 429, 5xx, timeouts, network failures and `409 concurrent_idempotent_requests`, on requests safe to repeat; `retry-after` honoured up to a minute, exponential backoff from 0.5 s to 8 s with jitter otherwise; per-attempt timeouts.
- List query parameters go comma-separated in one key, as the document declares them.
- `into_stream()` on every cursor-paginated list, forward from `after` or backward from `before`. A page without a `data` array ends the stream with an `application_error`; so does a page with `has_more` whose edge item has no `id`, after its items.
- Inbox event streams (server-sent events) as `futures` streams of `InboxStreamItem`, resuming with `Last-Event-ID` after a dropped connection; `stream.gap` handed out once per position; one inbox's stream ends after its `inbox.deleted` or `inbox.expired`.
- `verify_webhook` / `WebhookVerifier` check a webhook delivery's signature and timestamp and return the parsed event.
- Configuration from `BILLIONSEND_API_KEY` and `BILLIONSEND_BASE_URL`; rustls by default, `native-tls` as a feature; Rust 1.85 or later.
