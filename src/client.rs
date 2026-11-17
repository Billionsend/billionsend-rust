use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::error::{Error, Result};

/// The API this client calls unless told otherwise.
pub const DEFAULT_BASE_URL: &str = "https://api.billionsend.com";
/// The per-attempt timeout unless told otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// Retries of a failed request unless told otherwise.
pub const DEFAULT_MAX_RETRIES: u32 = 2;

/// The BillionSend API client.
///
/// Cheap to clone: clones share one connection pool. Every method returns a
/// request builder; await it to send the request.
///
/// ```no_run
/// # async fn run() -> Result<(), billionsend::Error> {
/// use serde_json::json;
///
/// let bs = billionsend::BillionSend::new("bs_...");
/// let email = bs
///     .emails()
///     .send(&json!({
///         "from": "Acme <hello@acme.com>",
///         "to": "user@example.com",
///         "subject": "Hello",
///         "html": "<p>Hi</p>",
///     }))
///     .await?;
/// println!("{}", email["id"]);
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct BillionSend {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) api_key: Option<String>,
    pub(crate) base_url: String,
    pub(crate) timeout: Duration,
    pub(crate) max_retries: u32,
    pub(crate) headers: HeaderMap,
    pub(crate) http: reqwest::Client,
}

impl std::fmt::Debug for BillionSend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BillionSend")
            .field("base_url", &self.inner.base_url)
            .field("api_key", &self.inner.api_key.as_ref().map(|_| "<set>"))
            .field("timeout", &self.inner.timeout)
            .field("max_retries", &self.inner.max_retries)
            .finish_non_exhaustive()
    }
}

fn read_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl BillionSend {
    /// A client with this API key (`bs_...`). The base URL is
    /// `BILLIONSEND_BASE_URL` when set, otherwise `https://api.billionsend.com`.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::builder()
            .api_key(api_key)
            .build()
            .expect("a client with default settings always builds")
    }

    /// A client configured from `BILLIONSEND_API_KEY` and `BILLIONSEND_BASE_URL`.
    /// Without a key every call fails with [`Error::Authentication`]
    /// (`missing_api_key`), except `agent().signup()`, which needs none.
    pub fn from_env() -> Self {
        Self::builder()
            .build()
            .expect("a client with default settings always builds")
    }

    /// A builder for the other settings: timeout, retries, headers, HTTP client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// The API base URL this client calls.
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    /// Whether an API key is configured.
    pub fn has_api_key(&self) -> bool {
        self.inner.api_key.is_some()
    }
}

/// Settings for a [`BillionSend`] client.
#[derive(Default)]
pub struct ClientBuilder {
    api_key: Option<String>,
    base_url: Option<String>,
    timeout: Option<Duration>,
    max_retries: Option<u32>,
    headers: Vec<(String, String)>,
    http: Option<reqwest::Client>,
}

impl std::fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

impl ClientBuilder {
    /// The API key (`bs_...`). Defaults to the `BILLIONSEND_API_KEY` environment variable.
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Defaults to `BILLIONSEND_BASE_URL`, then `https://api.billionsend.com`.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Per-attempt timeout (default 60 s). Event streams are timed only until they connect.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Retries for 429, 5xx, timeouts and network failures (default 2). Only
    /// requests safe to repeat are retried: `GET`, `PUT` and `DELETE`, and the
    /// sending operations, which always carry an `Idempotency-Key`.
    pub fn max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// An extra header sent with every request.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The `reqwest` client to send requests with (proxies, TLS roots, …).
    /// Its own timeout, if any, applies on top of this client's.
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http = Some(client);
        self
    }

    /// Builds the client. Fails only on a zero timeout or a header that is not valid HTTP.
    pub fn build(self) -> Result<BillionSend> {
        let timeout = self.timeout.unwrap_or(DEFAULT_TIMEOUT);
        if timeout.is_zero() {
            return Err(Error::application("timeout must be greater than zero"));
        }
        let mut headers = HeaderMap::new();
        for (name, value) in self.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| Error::application(format!("Invalid header name {name:?}")))?;
            let value = HeaderValue::from_str(&value)
                .map_err(|_| Error::application(format!("Invalid value for header {name}")))?;
            headers.insert(name, value);
        }
        let base_url = self
            .base_url
            .or_else(|| read_env("BILLIONSEND_BASE_URL"))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned())
            .trim_end_matches('/')
            .to_owned();
        Ok(BillionSend {
            inner: Arc::new(Inner {
                api_key: self.api_key.or_else(|| read_env("BILLIONSEND_API_KEY")),
                base_url,
                timeout,
                max_retries: self.max_retries.unwrap_or(DEFAULT_MAX_RETRIES),
                headers,
                http: self.http.unwrap_or_default(),
            }),
        })
    }
}
