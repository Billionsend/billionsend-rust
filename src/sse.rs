//! Inbox event streams (server-sent events).

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::request::Request;
use crate::retry::retryable;

/// The event name of a [`StreamGapEvent`].
pub const STREAM_GAP_EVENT: &str = "stream.gap";

/// One inbox event. `id` is its stream position: keep the last one to resume
/// with `last_event_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct InboxStreamEvent {
    /// The stream position of this event.
    pub id: String,
    /// The event name: `inbox.email.received`, `inbox.thread.created`,
    /// `email.received`, … (the inbox webhook events' names).
    pub event: String,
    /// The event's JSON body: `type`, `created_at`, `inbox_id` and `data`, the
    /// webhook payload's metadata (ids to read next with the REST methods,
    /// never a body, code or link).
    pub data: Value,
}

/// The stream resumed from a position older than the server keeps (24 hours):
/// events may be missing, so read the REST lists again. It does not move the
/// stream position. One stream hands it out once per position: a later
/// reconnect from the same position, which the server answers with another
/// gap, does not repeat it.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamGapEvent {
    /// Why the stream has a gap.
    pub reason: String,
}

/// What an inbox [`EventStream`] yields.
#[derive(Debug, Clone, PartialEq)]
pub enum InboxStreamItem {
    /// An inbox event.
    Event(InboxStreamEvent),
    /// Events may be missing (`stream.gap`).
    Gap(StreamGapEvent),
}

/// One server-sent event, as the parser reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSentEvent {
    /// The stream position after this event (the last `id` field seen), used to resume.
    pub id: Option<String>,
    /// The event name (`message` when the server sent none).
    pub event: String,
    /// `data` parsed as JSON; the raw text as a JSON string when it is not JSON.
    pub data: Value,
    /// The raw `data` text.
    pub raw: String,
}

/// An incremental `text/event-stream` parser (UTF-8, WHATWG rules).
#[derive(Debug, Default)]
pub struct EventParser {
    buffer: Vec<u8>,
    started: bool,
    last_id: Option<String>,
    event: String,
    data: Vec<String>,
    has_data: bool,
    retry: Option<u64>,
}

impl EventParser {
    /// A parser whose events carry `last_id` until the stream sends an `id`.
    pub fn new(last_id: Option<String>) -> Self {
        Self {
            last_id,
            ..Self::default()
        }
    }

    /// The last `retry` field the stream sent, in milliseconds, taken once.
    pub fn take_retry(&mut self) -> Option<u64> {
        self.retry.take()
    }

    /// Feeds a chunk of the body and returns the events it completes.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<ServerSentEvent> {
        self.buffer.extend_from_slice(chunk);
        if !self.started && !self.buffer.is_empty() {
            if self.buffer.len() < 3 && b"\xEF\xBB\xBF".starts_with(&self.buffer) {
                return Vec::new();
            }
            if self.buffer.starts_with(b"\xEF\xBB\xBF") {
                self.buffer.drain(..3);
            }
            self.started = true;
        }
        self.lines(false)
    }

    /// Ends the body: a last line without a line break is read, and an event
    /// not ended by a blank line is discarded, as the rules say.
    pub fn finish(&mut self) -> Vec<ServerSentEvent> {
        let events = self.lines(true);
        if !self.buffer.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).into_owned();
            // Nothing can follow, so this line cannot complete an event.
            let _ = self.line(&line);
        }
        events
    }

    fn lines(&mut self, final_chunk: bool) -> Vec<ServerSentEvent> {
        let mut events = Vec::new();
        let mut start = 0;
        while let Some(offset) = self.buffer[start..]
            .iter()
            .position(|b| *b == b'\n' || *b == b'\r')
        {
            let end = start + offset;
            let mut next = end + 1;
            if self.buffer[end] == b'\r' {
                match self.buffer.get(end + 1) {
                    Some(b'\n') => next = end + 2,
                    // A lone "\r" at the end may be the first half of "\r\n".
                    None if !final_chunk => break,
                    _ => {}
                }
            }
            let line = String::from_utf8_lossy(&self.buffer[start..end]).into_owned();
            if let Some(event) = self.line(&line) {
                events.push(event);
            }
            start = next;
        }
        self.buffer.drain(..start);
        events
    }

    fn line(&mut self, line: &str) -> Option<ServerSentEvent> {
        if line.is_empty() {
            if !self.has_data {
                self.event.clear();
                return None;
            }
            let raw = self.data.join("\n");
            let data = serde_json::from_str(&raw).unwrap_or_else(|_| Value::String(raw.clone()));
            let event = if self.event.is_empty() {
                "message".to_owned()
            } else {
                std::mem::take(&mut self.event)
            };
            self.event.clear();
            self.data.clear();
            self.has_data = false;
            return Some(ServerSentEvent {
                id: self.last_id.clone(),
                event,
                data,
                raw,
            });
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.find(':') {
            Some(i) => (
                &line[..i],
                line[i + 1..].strip_prefix(' ').unwrap_or(&line[i + 1..]),
            ),
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_owned(),
            "data" => {
                self.data.push(value.to_owned());
                self.has_data = true;
            }
            "id" if !value.contains('\0') => self.last_id = Some(value.to_owned()),
            "retry" if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
                self.retry = value.parse().ok();
            }
            _ => {}
        }
        None
    }
}

/// How an [`EventStream`] reconnects.
#[derive(Debug, Clone)]
pub(crate) struct StreamOptions {
    pub(crate) last_event_id: Option<String>,
    pub(crate) reconnect: bool,
    pub(crate) max_reconnects: u32,
    pub(crate) reconnect_delay: Duration,
    pub(crate) max_retries: Option<u32>,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            last_event_id: None,
            reconnect: true,
            max_reconnects: 5,
            reconnect_delay: Duration::from_millis(1000),
            max_retries: None,
        }
    }
}

type Body = Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>;

struct State {
    request: Request,
    body: Option<Body>,
    parser: EventParser,
    pending: VecDeque<ServerSentEvent>,
    position: Arc<Mutex<Option<String>>>,
    delay: Duration,
    reconnect: bool,
    max_reconnects: u32,
    failures: u32,
    gap_at: Option<Option<String>>,
    ends_on: &'static [&'static str],
    done: bool,
}

impl State {
    fn position(&self) -> Option<String> {
        self.position.lock().expect("stream position lock").clone()
    }

    fn take_events(&mut self, events: Vec<ServerSentEvent>) {
        if let Some(ms) = self.parser.take_retry() {
            self.delay = Duration::from_millis(ms);
        }
        self.pending.extend(events);
    }
}

fn with_position(request: &Request, position: Option<&str>) -> Request {
    let mut request = request.clone();
    if let Some(id) = position {
        request.set_header("last-event-id", id);
    }
    request
}

async fn next(mut state: State) -> Option<(Result<InboxStreamItem>, State)> {
    loop {
        if state.done {
            return None;
        }
        if let Some(event) = state.pending.pop_front() {
            state.failures = 0;
            if event.event == STREAM_GAP_EVENT {
                let position = state.position();
                if state.gap_at.as_ref() == Some(&position) {
                    continue;
                }
                state.gap_at = Some(position);
                let reason = event
                    .data
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                return Some((Ok(InboxStreamItem::Gap(StreamGapEvent { reason })), state));
            }
            *state.position.lock().expect("stream position lock") = event.id.clone();
            if state.ends_on.contains(&event.event.as_str()) {
                state.done = true;
                state.body = None;
            }
            let item = InboxStreamEvent {
                id: event.id.unwrap_or_default(),
                event: event.event,
                data: event.data,
            };
            return Some((Ok(InboxStreamItem::Event(item)), state));
        }
        if let Some(body) = state.body.as_mut() {
            match body.next().await {
                Some(Ok(chunk)) => {
                    let events = state.parser.feed(&chunk);
                    state.take_events(events);
                }
                Some(Err(_)) | None => {
                    // The connection ended or dropped; finish what it sent, then reconnect.
                    state.body = None;
                    let events = state.parser.finish();
                    state.take_events(events);
                }
            }
            continue;
        }
        if !state.reconnect {
            return None;
        }
        state.failures += 1;
        if state.failures > state.max_reconnects {
            state.done = true;
            let error = Error::connection(format!(
                "The event stream disconnected; gave up after {} reconnects",
                state.max_reconnects
            ));
            return Some((Err(error), state));
        }
        tokio::time::sleep(state.delay).await;
        let position = state.position();
        let request = with_position(&state.request, position.as_deref());
        match request.execute_stream(Some(0)).await {
            Ok(response) => {
                state.parser = EventParser::new(position);
                state.body = Some(Box::pin(response.bytes_stream()));
            }
            Err(error) if !retryable(&error, false) => {
                state.done = true;
                return Some((Err(error), state));
            }
            Err(_) => {}
        }
    }
}

/// A live inbox event stream: poll it as a [`Stream`] of [`InboxStreamItem`]s;
/// dropping it closes the connection. When the connection drops (the server
/// also closes every stream about every ten minutes) it reconnects with
/// `Last-Event-ID`, and the events in between are replayed first, unless
/// `reconnect(false)` was set. After `max_reconnects` failures in a row it
/// yields an [`Error::Connection`] and ends; an error a retry cannot fix (a
/// revoked key, say) is yielded as itself and ends the stream.
pub struct EventStream {
    inner: Pin<Box<dyn Stream<Item = Result<InboxStreamItem>> + Send>>,
    position: Arc<Mutex<Option<String>>>,
    headers: reqwest::header::HeaderMap,
}

impl std::fmt::Debug for EventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventStream")
            .field("last_event_id", &self.last_event_id())
            .finish_non_exhaustive()
    }
}

impl EventStream {
    pub(crate) async fn open(
        request: Request,
        options: StreamOptions,
        ends_on: &'static [&'static str],
    ) -> Result<Self> {
        let first = with_position(&request, options.last_event_id.as_deref());
        let response = first.execute_stream(options.max_retries).await?;
        let headers = response.headers().clone();
        let position = Arc::new(Mutex::new(options.last_event_id.clone()));
        let state = State {
            request,
            body: Some(Box::pin(response.bytes_stream())),
            parser: EventParser::new(options.last_event_id),
            pending: VecDeque::new(),
            position: Arc::clone(&position),
            delay: options.reconnect_delay,
            reconnect: options.reconnect,
            max_reconnects: options.max_reconnects,
            failures: 0,
            gap_at: None,
            ends_on,
            done: false,
        };
        let inner = Box::pin(futures_util::stream::unfold(state, next));
        Ok(Self {
            inner,
            position,
            headers,
        })
    }

    /// The id of the last event received: pass it to `last_event_id` to resume later.
    pub fn last_event_id(&self) -> Option<String> {
        self.position.lock().expect("stream position lock").clone()
    }

    /// The headers of the first connection's answer.
    pub fn headers(&self) -> &reqwest::header::HeaderMap {
        &self.headers
    }
}

impl Stream for EventStream {
    type Item = Result<InboxStreamItem>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// The methods of the two event-stream builders.
macro_rules! stream_builder {
    ($name:ident, $ends_on:expr) => {
        impl $name {
            /// Adds a header to the stream request.
            pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
                self.inner.set_header(name, value);
                self
            }

            /// The timeout of each connection attempt until the stream opens
            /// (defaults to the client's timeout).
            pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
                self.inner.set_timeout(timeout);
                self
            }

            /// Retries of the first connection on 429, 5xx and network failures
            /// (defaults to the client's `max_retries`).
            pub fn max_retries(mut self, max_retries: u32) -> Self {
                self.options.max_retries = Some(max_retries);
                self
            }

            /// Resume after this event id (sent as the `Last-Event-ID` header).
            pub fn last_event_id(mut self, id: impl Into<String>) -> Self {
                self.options.last_event_id = Some(id.into());
                self
            }

            /// Reconnect with `Last-Event-ID` when the connection drops (default `true`).
            pub fn reconnect(mut self, reconnect: bool) -> Self {
                self.options.reconnect = reconnect;
                self
            }

            /// Reconnect attempts in a row before giving up (default 5).
            pub fn max_reconnects(mut self, max_reconnects: u32) -> Self {
                self.options.max_reconnects = max_reconnects;
                self
            }

            /// Wait before reconnecting (default 1 s; a server `retry` field overrides it).
            pub fn reconnect_delay(mut self, delay: std::time::Duration) -> Self {
                self.options.reconnect_delay = delay;
                self
            }
        }

        impl std::future::IntoFuture for $name {
            type Output = $crate::Result<$crate::sse::EventStream>;
            type IntoFuture = std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = $crate::Result<$crate::sse::EventStream>>
                        + Send,
                >,
            >;

            fn into_future(self) -> Self::IntoFuture {
                Box::pin($crate::sse::EventStream::open(
                    self.inner,
                    self.options,
                    $ends_on,
                ))
            }
        }
    };
}

pub(crate) use stream_builder;

/// The events after which one inbox's stream is over (the server closes it,
/// and a reconnect would answer 404).
pub(crate) const INBOX_GONE_EVENTS: &[&str] = &["inbox.deleted", "inbox.expired"];
