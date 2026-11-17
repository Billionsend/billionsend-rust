# Changelog

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
