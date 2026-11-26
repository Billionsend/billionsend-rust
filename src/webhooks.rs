//! Webhook signature verification.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::error::{Error, Result};

/// The default tolerance between the `webhook-timestamp` and now.
pub const WEBHOOK_TOLERANCE: Duration = Duration::from_secs(300);

const SECRET_PREFIX: &str = "whsec_";
const SECRET_MIN_BYTES: usize = 24;
const SECRET_MAX_BYTES: usize = 64;

/// Request headers a webhook can be read from, by case-insensitive name.
pub trait WebhookHeaders {
    /// The first value of `name` (lower case), if present.
    fn header(&self, name: &str) -> Option<String>;
}

impl WebhookHeaders for reqwest::header::HeaderMap {
    fn header(&self, name: &str) -> Option<String> {
        self.get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }
}

fn find<'a, K: AsRef<str> + 'a, V: AsRef<str> + 'a>(
    pairs: impl IntoIterator<Item = (&'a K, &'a V)>,
    name: &str,
) -> Option<String> {
    pairs
        .into_iter()
        .find(|(k, _)| k.as_ref().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_ref().to_owned())
}

impl<K: AsRef<str>, V: AsRef<str>, S> WebhookHeaders for HashMap<K, V, S> {
    fn header(&self, name: &str) -> Option<String> {
        find(self.iter(), name)
    }
}

impl<K: AsRef<str>, V: AsRef<str>> WebhookHeaders for BTreeMap<K, V> {
    fn header(&self, name: &str) -> Option<String> {
        find(self.iter(), name)
    }
}

impl<K: AsRef<str>, V: AsRef<str>> WebhookHeaders for [(K, V)] {
    fn header(&self, name: &str) -> Option<String> {
        find(self.iter().map(|(k, v)| (k, v)), name)
    }
}

impl<K: AsRef<str>, V: AsRef<str>, const N: usize> WebhookHeaders for [(K, V); N] {
    fn header(&self, name: &str) -> Option<String> {
        self.as_slice().header(name)
    }
}

impl<K: AsRef<str>, V: AsRef<str>> WebhookHeaders for Vec<(K, V)> {
    fn header(&self, name: &str) -> Option<String> {
        self.as_slice().header(name)
    }
}

impl<T: WebhookHeaders + ?Sized> WebhookHeaders for &T {
    fn header(&self, name: &str) -> Option<String> {
        (**self).header(name)
    }
}

/// Verifies webhook deliveries for one endpoint secret.
///
/// ```
/// # fn run(raw_body: &[u8], headers: &reqwest::header::HeaderMap) -> Result<(), billionsend::Error> {
/// use billionsend::webhooks::WebhookVerifier;
///
/// let verifier = WebhookVerifier::new("whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw")?;
/// let event = verifier.verify(raw_body, headers)?;
/// if event["type"] == "inbox.email.received" {
///     // ...
/// }
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct WebhookVerifier {
    key: Vec<u8>,
    tolerance: Duration,
    now: Option<SystemTime>,
}

impl std::fmt::Debug for WebhookVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookVerifier")
            .field("tolerance", &self.tolerance)
            .finish_non_exhaustive()
    }
}

impl WebhookVerifier {
    /// A verifier for a `whsec_` secret (canonical base64 of 24 to 64 bytes, as
    /// the dashboard and the API hand it out).
    pub fn new(secret: &str) -> Result<Self> {
        let malformed = || Error::webhook("The webhook secret is not a whsec_ secret");
        // A secret read from a file ends in a newline.
        let encoded = secret
            .trim()
            .strip_prefix(SECRET_PREFIX)
            .ok_or_else(malformed)?;
        let key = STANDARD.decode(encoded).map_err(|_| malformed())?;
        if !(SECRET_MIN_BYTES..=SECRET_MAX_BYTES).contains(&key.len())
            || STANDARD.encode(&key) != encoded
        {
            return Err(malformed());
        }
        Ok(Self {
            key,
            tolerance: WEBHOOK_TOLERANCE,
            now: None,
        })
    }

    /// How far the `webhook-timestamp` may be from now (default 300 s).
    pub fn tolerance(mut self, tolerance: Duration) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// The time to check the timestamp against (default: the clock).
    pub fn now(mut self, now: SystemTime) -> Self {
        self.now = Some(now);
        self
    }

    /// Verifies a delivery and returns its parsed JSON body.
    ///
    /// The signature is HMAC-SHA256 over `<webhook-id>.<webhook-timestamp>.<body>`
    /// with the endpoint's secret, sent as `v1,<base64>` in `webhook-signature`;
    /// during a secret rotation the header carries one signature per secret,
    /// space-separated, and any of them matching is enough. The timestamp (Unix
    /// seconds) must be within the tolerance of now, so a captured delivery
    /// cannot be replayed later.
    ///
    /// Pass the body exactly as it arrived, before any JSON parsing:
    /// re-serialised JSON does not verify.
    ///
    /// Fails with [`Error::WebhookVerification`] when a header is missing, the
    /// timestamp is out of tolerance, no signature matches or the body is not JSON.
    pub fn verify(
        &self,
        payload: impl AsRef<[u8]>,
        headers: &(impl WebhookHeaders + ?Sized),
    ) -> Result<serde_json::Value> {
        let body = payload.as_ref();
        let read = |name| {
            headers
                .header(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let (Some(id), Some(timestamp), Some(signature)) = (
            read("webhook-id"),
            read("webhook-timestamp"),
            read("webhook-signature"),
        ) else {
            return Err(Error::webhook(
                "Missing the webhook-id, webhook-timestamp or webhook-signature header",
            ));
        };
        if !timestamp.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::webhook("Invalid webhook-timestamp header"));
        }
        let seconds: i128 = timestamp
            .parse()
            .map_err(|_| Error::webhook("Invalid webhook-timestamp header"))?;
        let now = self.now.unwrap_or_else(SystemTime::now);
        let now = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i128)
            .unwrap_or(0);
        let tolerance = self.tolerance.as_secs() as i128;
        if now - seconds > tolerance {
            return Err(Error::webhook("The webhook timestamp is too old"));
        }
        if seconds - now > tolerance {
            return Err(Error::webhook("The webhook timestamp is too new"));
        }

        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.key)
            .expect("HMAC takes any key length");
        mac.update(id.as_bytes());
        mac.update(b".");
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        let matched = signature.split_whitespace().any(|candidate| {
            let Some((version, encoded)) = candidate.split_once(',') else {
                return false;
            };
            if version != "v1" {
                return false;
            }
            match STANDARD.decode(encoded) {
                Ok(given) => mac.clone().verify_slice(&given).is_ok(),
                Err(_) => false,
            }
        });
        if !matched {
            return Err(Error::webhook("No webhook signature matches"));
        }
        serde_json::from_slice(body).map_err(|_| Error::webhook("The webhook body is not JSON"))
    }
}

/// Verifies a webhook delivery with the default tolerance and returns its parsed body.
/// See [`WebhookVerifier::verify`].
///
/// ```no_run
/// # fn run(raw_body: &[u8], headers: &reqwest::header::HeaderMap) -> Result<(), billionsend::Error> {
/// let secret = std::env::var("BILLIONSEND_WEBHOOK_SECRET").unwrap();
/// let event = billionsend::verify_webhook(raw_body, headers, &secret)?;
/// println!("{}", event["type"]);
/// # Ok(()) }
/// ```
pub fn verify_webhook(
    payload: impl AsRef<[u8]>,
    headers: &(impl WebhookHeaders + ?Sized),
    secret: &str,
) -> Result<serde_json::Value> {
    WebhookVerifier::new(secret)?.verify(payload, headers)
}
