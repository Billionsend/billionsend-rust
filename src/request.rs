use std::time::{Duration, SystemTime};

use bytes::Bytes;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::Method;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::client::BillionSend;
use crate::error::{Error, Result};
use crate::retry::{idempotent_method, new_idempotency_key, retry_delay, retryable};
use crate::upload::{FileKind, FileUpload, Form};

/// The characters `encodeURIComponent` leaves alone stay; everything else is escaped.
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

const IDEMPOTENCY_KEY: &str = "idempotency-key";

/// A successful answer with its status and headers, from `.raw()`.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    /// The HTTP status.
    pub status: u16,
    /// The response headers.
    pub headers: HeaderMap,
    /// The parsed JSON body.
    pub body: serde_json::Value,
}

impl ApiResponse {
    /// The answer's `x-request-id`.
    pub fn request_id(&self) -> Option<&str> {
        self.headers
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
    }
}

/// One operation's request, before it is sent. The generated builders wrap it.
#[derive(Clone, Debug)]
pub(crate) struct Request {
    client: BillionSend,
    method: Method,
    path: String,
    query: Vec<(String, String)>,
    body: Option<Bytes>,
    /// An upload's file and fields, encoded into `body` when sent.
    form: Option<Form>,
    headers: Vec<(String, String)>,
    idempotent: bool,
    idempotency_key: Option<String>,
    auth: bool,
    timeout: Option<Duration>,
    max_retries: Option<u32>,
    error: Option<Error>,
}

/// The raw answer of one attempt.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
}

/// An open answer whose body is still arriving (an event stream).
pub(crate) struct Opened {
    pub(crate) response: reqwest::Response,
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn describe(error: &reqwest::Error) -> String {
    use std::error::Error as _;
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        source = inner.source();
    }
    text
}

impl Request {
    /// A request for `template` with its `{name}` placeholders filled from
    /// `params` (escaped as one path segment each). A missing or empty value
    /// makes the request fail when sent, without a call.
    pub(crate) fn new(
        client: &BillionSend,
        method: Method,
        template: &str,
        params: &[(&str, &str)],
    ) -> Self {
        let mut path = String::with_capacity(template.len() + 32);
        let mut error = None;
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            path.push_str(&rest[..open]);
            let close = rest[open..]
                .find('}')
                .map(|i| open + i)
                .expect("closed placeholder");
            let name = &rest[open + 1..close];
            let value = params.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
            match value {
                // A URL folds these away, and the request would reach another route.
                Some(v @ ("." | "..")) => {
                    error.get_or_insert_with(|| {
                        Error::application(format!("Invalid path parameter \"{name}\": \"{v}\""))
                    });
                }
                Some(v) if !v.is_empty() => path.extend(utf8_percent_encode(v, PATH_SEGMENT)),
                _ => {
                    error.get_or_insert_with(|| {
                        Error::application(format!("Missing path parameter \"{name}\""))
                    });
                }
            }
            rest = &rest[close + 1..];
        }
        path.push_str(rest);
        Self {
            client: client.clone(),
            method,
            path,
            query: Vec::new(),
            body: None,
            form: None,
            headers: Vec::new(),
            idempotent: false,
            idempotency_key: None,
            auth: true,
            timeout: None,
            max_retries: None,
            error,
        }
    }

    /// The operation honours `Idempotency-Key`: one is always sent, the
    /// caller's or a new one that this call's retries reuse.
    pub(crate) fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }

    /// The few operations that take no API key.
    pub(crate) fn without_auth(mut self) -> Self {
        self.auth = false;
        self
    }

    pub(crate) fn json<B: Serialize + ?Sized>(mut self, body: &B) -> Self {
        match serde_json::to_vec(body) {
            Ok(bytes) => self.body = Some(bytes.into()),
            Err(e) => {
                self.error.get_or_insert_with(|| {
                    Error::application(format!("The request body does not serialise: {e}"))
                });
            }
        }
        self
    }

    /// Makes this an upload: `multipart/form-data` with this file.
    pub(crate) fn file(mut self, field: &'static str, file: FileUpload, kind: FileKind) -> Self {
        self.form = Some(Form::new(field, file, kind));
        self
    }

    /// A text field of an upload.
    pub(crate) fn set_field(&mut self, name: &'static str, value: String) {
        self.form
            .as_mut()
            .expect("only uploads have fields")
            .set_field(name, value);
    }

    /// A field of an upload the API reads as JSON: a string is sent as it is
    /// (already encoded), anything else is encoded.
    pub(crate) fn set_json_field<T: Serialize + ?Sized>(&mut self, name: &'static str, value: &T) {
        match serde_json::to_string(value) {
            Ok(json) => {
                let text = serde_json::from_str::<String>(&json).unwrap_or(json);
                self.set_field(name, text);
            }
            Err(e) => {
                self.error.get_or_insert_with(|| {
                    Error::application(format!("The {name} field does not serialise: {e}"))
                });
            }
        }
    }

    pub(crate) fn set_query(&mut self, name: &str, value: String) {
        self.query.retain(|(n, _)| n != name);
        self.query.push((name.to_owned(), value));
    }

    pub(crate) fn query_value(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn remove_query(&mut self, name: &str) {
        self.query.retain(|(n, _)| n != name);
    }

    pub(crate) fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into().to_ascii_lowercase();
        self.headers.retain(|(n, _)| *n != name);
        self.headers.push((name, value.into()));
    }

    pub(crate) fn set_idempotency_key(&mut self, key: impl Into<String>) {
        self.idempotency_key = Some(key.into());
    }

    pub(crate) fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = Some(timeout);
    }

    pub(crate) fn set_max_retries(&mut self, max_retries: u32) {
        self.max_retries = Some(max_retries);
    }

    /// The headers of every attempt, and whether they carry an idempotency key.
    fn build_headers(&self, stream: bool) -> Result<(HeaderMap, bool)> {
        let inner = &self.client.inner;
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static(if stream {
                "text/event-stream"
            } else {
                "application/json"
            }),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("billionsend-rust/", env!("CARGO_PKG_VERSION"))),
        );
        for (name, value) in &inner.headers {
            headers.insert(name.clone(), value.clone());
        }
        for (name, value) in &self.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| Error::invalid_header(format!("Invalid header name {name:?}")))?;
            let value = HeaderValue::from_str(value)
                .map_err(|_| Error::invalid_header(format!("Invalid value for header {name}")))?;
            headers.insert(name, value);
        }
        if let Some(form) = &self.form {
            let value = HeaderValue::from_str(&form.content_type()).expect("an ASCII content type");
            headers.insert(CONTENT_TYPE, value);
        } else if self.body.is_some() {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        let key = match &self.idempotency_key {
            Some(key) => Some(key.clone()),
            None if self.idempotent && !headers.contains_key(IDEMPOTENCY_KEY) => {
                Some(new_idempotency_key())
            }
            None => None,
        };
        if let Some(key) = key {
            let value = HeaderValue::from_str(&key)
                .map_err(|_| Error::invalid_header("Invalid value for header idempotency-key"))?;
            headers.insert(IDEMPOTENCY_KEY, value);
        }
        if self.auth {
            let Some(api_key) = &inner.api_key else {
                return Err(Error::Authentication(crate::error::ErrorDetails {
                    name: "missing_api_key".to_owned(),
                    ..crate::error::ErrorDetails::application(
                        "No API key: pass one to BillionSend::new or set BILLIONSEND_API_KEY",
                    )
                }));
            };
            let value = HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|_| Error::invalid_header("The API key is not a valid header value"))?;
            headers.insert(AUTHORIZATION, value);
        } else {
            headers.remove(AUTHORIZATION);
        }
        // `HeaderValue` lets non-ASCII bytes through; the API does not take them.
        if let Some((name, _)) = headers.iter().find(|(_, v)| v.to_str().is_err()) {
            return Err(Error::invalid_header(format!(
                "Invalid value for header {name}"
            )));
        }
        let keyed = headers.contains_key(IDEMPOTENCY_KEY);
        Ok((headers, keyed))
    }

    fn url(&self) -> String {
        format!("{}{}", self.client.inner.base_url, self.path)
    }

    async fn attempt(&self, headers: &HeaderMap, timeout: Duration) -> Result<Answer> {
        let http = &self.client.inner.http;
        let mut builder = http
            .request(self.method.clone(), self.url())
            .headers(headers.clone());
        if !self.query.is_empty() {
            builder = builder.query(&self.query);
        }
        if let Some(body) = &self.body {
            builder = builder.body(body.clone());
        }
        let work = async {
            let response = builder.send().await?;
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let body = response.bytes().await?;
            Ok::<_, reqwest::Error>(Answer {
                status,
                headers,
                body,
            })
        };
        match tokio::time::timeout(timeout, work).await {
            Err(_) => Err(Error::timeout(format!(
                "Request timed out after {} ms",
                timeout.as_millis()
            ))),
            Ok(Err(e)) if e.is_timeout() => Err(Error::timeout(format!(
                "Request timed out: {}",
                describe(&e)
            ))),
            Ok(Err(e)) => Err(Error::connection(format!(
                "Network error: {}",
                describe(&e)
            ))),
            Ok(Ok(answer)) => Ok(answer),
        }
    }

    async fn open(&self, headers: &HeaderMap, timeout: Duration) -> Result<Opened> {
        let http = &self.client.inner.http;
        let mut builder = http
            .request(self.method.clone(), self.url())
            .headers(headers.clone());
        if !self.query.is_empty() {
            builder = builder.query(&self.query);
        }
        match tokio::time::timeout(timeout, builder.send()).await {
            Err(_) => Err(Error::timeout(format!(
                "Request timed out after {} ms",
                timeout.as_millis()
            ))),
            Ok(Err(e)) if e.is_timeout() => Err(Error::timeout(format!(
                "Request timed out: {}",
                describe(&e)
            ))),
            Ok(Err(e)) => Err(Error::connection(format!(
                "Network error: {}",
                describe(&e)
            ))),
            Ok(Ok(response)) => Ok(Opened { response }),
        }
    }

    fn failure(answer: &Answer, reason: Option<&str>) -> Error {
        Error::from_response(
            answer.status,
            reason,
            &answer.body,
            header_str(&answer.headers, "x-request-id"),
            header_str(&answer.headers, "retry-after").as_deref(),
        )
    }

    /// Sends the request, retrying what is safe to retry, and returns the 2xx answer.
    pub(crate) async fn execute(mut self) -> Result<Answer> {
        if let Some(error) = self.error.clone() {
            return Err(error);
        }
        let (headers, keyed) = self.build_headers(false)?;
        if let Some(form) = &self.form {
            self.body = Some(form.encode()?);
        }
        let inner = &self.client.inner;
        // An upload is not repeated blindly, whatever its method.
        let may_retry = keyed || (idempotent_method(&self.method) && self.form.is_none());
        let max_retries = self.max_retries.unwrap_or(inner.max_retries);
        let timeout = self.timeout.unwrap_or(inner.timeout);
        let mut attempt = 0u32;
        loop {
            let (error, retry_after) = match self.attempt(&headers, timeout).await {
                Ok(answer) if (200..300).contains(&answer.status) => return Ok(answer),
                Ok(answer) => {
                    let reason = reqwest::StatusCode::from_u16(answer.status)
                        .ok()
                        .and_then(|s| s.canonical_reason());
                    (
                        Self::failure(&answer, reason),
                        header_str(&answer.headers, "retry-after"),
                    )
                }
                Err(error) => (error, None),
            };
            // A 429 is answered before anything is written: any request may repeat after one.
            let repeatable = may_retry || matches!(error, Error::RateLimit(_));
            if !repeatable || attempt >= max_retries || !retryable(&error, keyed) {
                return Err(error);
            }
            let Some(delay) = retry_delay(
                attempt,
                retry_after.as_deref(),
                SystemTime::now(),
                fastrand::f64(),
            ) else {
                return Err(error);
            };
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }

    /// Opens an event stream, retrying the connection like any `GET`; the
    /// timeout covers only the wait for the response headers.
    pub(crate) async fn execute_stream(
        self,
        max_retries: Option<u32>,
    ) -> Result<reqwest::Response> {
        if let Some(error) = self.error.clone() {
            return Err(error);
        }
        let (headers, _) = self.build_headers(true)?;
        let inner = &self.client.inner;
        let max_retries = max_retries
            .or(self.max_retries)
            .unwrap_or(inner.max_retries);
        let timeout = self.timeout.unwrap_or(inner.timeout);
        let mut attempt = 0u32;
        loop {
            let (error, retry_after) = match self.open(&headers, timeout).await {
                Ok(Opened { response }) if response.status().is_success() => {
                    let content_type =
                        header_str(response.headers(), "content-type").unwrap_or_default();
                    if content_type.contains("text/event-stream") {
                        return Ok(response);
                    }
                    let status = response.status().as_u16();
                    return Err(Error::Api(crate::error::ErrorDetails {
                        status_code: Some(status),
                        request_id: header_str(response.headers(), "x-request-id"),
                        ..crate::error::ErrorDetails::application(format!(
                            "Expected an event stream, got {}",
                            if content_type.is_empty() {
                                "no content type"
                            } else {
                                &content_type
                            }
                        ))
                    }));
                }
                Ok(Opened { response }) => {
                    let status = response.status();
                    let headers = response.headers().clone();
                    let body = response.bytes().await.unwrap_or_default();
                    let answer = Answer {
                        status: status.as_u16(),
                        headers,
                        body,
                    };
                    (
                        Self::failure(&answer, status.canonical_reason()),
                        header_str(&answer.headers, "retry-after"),
                    )
                }
                Err(error) => (error, None),
            };
            if attempt >= max_retries || !retryable(&error, false) {
                return Err(error);
            }
            let Some(delay) = retry_delay(
                attempt,
                retry_after.as_deref(),
                SystemTime::now(),
                fastrand::f64(),
            ) else {
                return Err(error);
            };
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }

    /// Sends the request and parses the answer as JSON into `T`.
    pub(crate) async fn send_json<T: DeserializeOwned>(self) -> Result<T> {
        let answer = self.execute().await?;
        parse(&answer)
    }

    /// Sends the request and keeps the status and headers of the answer.
    pub(crate) async fn send_raw(self) -> Result<ApiResponse> {
        let answer = self.execute().await?;
        let body = parse(&answer)?;
        Ok(ApiResponse {
            status: answer.status,
            headers: answer.headers,
            body,
        })
    }
}

/// The answer's JSON; a success with no body reads as `null`.
fn parse<T: DeserializeOwned>(answer: &Answer) -> Result<T> {
    let body: &[u8] = if answer.body.trim_ascii().is_empty() {
        b"null"
    } else {
        &answer.body
    };
    serde_json::from_slice(body).map_err(|e| {
        Error::Api(crate::error::ErrorDetails {
            status_code: Some(answer.status),
            request_id: header_str(&answer.headers, "x-request-id"),
            ..crate::error::ErrorDetails::application(format!(
                "The response body is not the expected JSON: {e}"
            ))
        })
    })
}

/// Joins a list parameter's values the way the API reads them: one key, comma-separated.
pub(crate) fn join_list<I, S>(values: I) -> String
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    values
        .into_iter()
        .map(Into::into)
        .collect::<Vec<_>>()
        .join(",")
}

/// The methods every generated request builder has.
macro_rules! request_builder {
    ($name:ident) => {
        impl $name {
            /// Adds a header to this request (replacing one of the same name).
            pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
                self.inner.set_header(name, value);
                self
            }

            /// Overrides the client's per-attempt timeout for this request.
            pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
                self.inner.set_timeout(timeout);
                self
            }

            /// Overrides the client's retry count for this request.
            pub fn max_retries(mut self, max_retries: u32) -> Self {
                self.inner.set_max_retries(max_retries);
                self
            }

            /// Sends the request and deserialises the answer into `T` instead of a JSON value.
            pub async fn json<T: serde::de::DeserializeOwned>(self) -> $crate::Result<T> {
                self.inner.send_json().await
            }

            /// Sends the request and returns the answer with its status and headers.
            pub async fn raw(self) -> $crate::Result<$crate::ApiResponse> {
                self.inner.send_raw().await
            }
        }

        impl std::future::IntoFuture for $name {
            type Output = $crate::Result<serde_json::Value>;
            type IntoFuture = std::pin::Pin<
                Box<dyn std::future::Future<Output = $crate::Result<serde_json::Value>> + Send>,
            >;

            fn into_future(self) -> Self::IntoFuture {
                Box::pin(self.inner.send_json())
            }
        }
    };
}

/// `idempotency_key` for the sending operations.
macro_rules! idempotent_builder {
    ($name:ident) => {
        impl $name {
            /// Sent as the `Idempotency-Key` header (1 to 256 characters): a
            /// repeat with the same key and payload answers the first result
            /// instead of sending again. Without one the client makes a new
            /// key for each call (a UUID), and that call's retries reuse it;
            /// pass your own to make the send safe across calls and processes
            /// too, such as `welcome/{user_id}`.
            pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
                self.inner.set_idempotency_key(key);
                self
            }
        }
    };
}

/// `into_stream` for the cursor-paginated lists.
macro_rules! paginated_builder {
    ($name:ident) => {
        impl $name {
            /// Every item of the list, fetched page by page as the stream is
            /// polled. Starting with `before` set walks backward; otherwise it
            /// walks forward from `after` (or the start). A failed page ends the
            /// stream with its error.
            pub fn into_stream(self) -> $crate::pagination::Paginate {
                $crate::pagination::paginate(self.inner)
            }
        }
    };
}

pub(crate) use {idempotent_builder, paginated_builder, request_builder};
