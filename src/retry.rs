//! When and how long the client waits before retrying.

use std::time::{Duration, SystemTime};

use crate::error::Error;

/// A `retry-after` longer than this is not waited for: the error is returned instead.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

const BASE_DELAY_MS: f64 = 500.0;
const MAX_BACKOFF_MS: f64 = 8_000.0;

/// Whether a method is safe to repeat without an `Idempotency-Key`.
pub(crate) fn idempotent_method(method: &reqwest::Method) -> bool {
    matches!(
        method.as_str(),
        "GET" | "HEAD" | "OPTIONS" | "PUT" | "DELETE"
    )
}

/// The wait a `retry-after` value asks for (seconds or an HTTP date), `None`
/// when it is absent or unreadable.
pub(crate) fn parse_retry_after(value: Option<&str>, now: SystemTime) -> Option<Duration> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<f64>() {
        if seconds.is_finite() {
            return Some(Duration::from_secs_f64(seconds.max(0.0)));
        }
        return None;
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(now).unwrap_or(Duration::ZERO))
}

/// How long to wait before retry number `attempt + 1`: the server's
/// `retry-after` when it sent one, otherwise an exponential backoff from 0.5 s,
/// capped at 8 s, less up to a quarter of jitter. `None` means "do not retry",
/// which is the answer for a `retry-after` longer than [`MAX_RETRY_AFTER`].
pub fn retry_delay(
    attempt: u32,
    retry_after: Option<&str>,
    now: SystemTime,
    random: f64,
) -> Option<Duration> {
    if let Some(wait) = parse_retry_after(retry_after, now) {
        if wait > MAX_RETRY_AFTER {
            return None;
        }
        // Whole milliseconds, rounded up, as the server's hint is a minimum.
        return Some(Duration::from_millis(
            (wait.as_secs_f64() * 1000.0).ceil() as u64
        ));
    }
    let backoff = (BASE_DELAY_MS * 2f64.powi(attempt.min(30) as i32)).min(MAX_BACKOFF_MS);
    Some(Duration::from_millis(
        (backoff * (1.0 - random * 0.25)).round() as u64,
    ))
}

/// Whether a failed attempt may be retried: a 429, a 5xx, a timeout or a
/// network failure; and, for a request that carries an `Idempotency-Key`, the
/// `409 concurrent_idempotent_requests` the API answers while the first attempt
/// with that key is still being processed (other 409s are final).
pub fn retryable(error: &Error, idempotency_key: bool) -> bool {
    match error {
        Error::Connection(_) | Error::Timeout(_) => true,
        Error::RateLimit(_) | Error::Server(_) => true,
        Error::Conflict(d) => idempotency_key && d.name == "concurrent_idempotent_requests",
        _ => false,
    }
}

/// A new idempotency key: a random UUID (version 4).
pub fn new_idempotency_key() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_half_a_second_and_caps_at_eight() {
        let now = SystemTime::now();
        let ms = |attempt| retry_delay(attempt, None, now, 0.0).unwrap().as_millis();
        assert_eq!(
            [ms(0), ms(1), ms(2), ms(3), ms(4), ms(5)],
            [500, 1000, 2000, 4000, 8000, 8000]
        );
        assert_eq!(retry_delay(0, None, now, 1.0).unwrap().as_millis(), 375);
    }

    #[test]
    fn retry_after_seconds_and_dates() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(
            retry_delay(0, Some("2"), now, 0.5),
            Some(Duration::from_millis(2000))
        );
        assert_eq!(
            retry_delay(0, Some("0.25"), now, 0.5),
            Some(Duration::from_millis(250))
        );
        assert_eq!(retry_delay(0, Some("61"), now, 0.5), None);
        let date = httpdate::fmt_http_date(now + Duration::from_secs(3));
        assert_eq!(
            retry_delay(0, Some(&date), now, 0.5),
            Some(Duration::from_millis(3000))
        );
        let past = httpdate::fmt_http_date(now - Duration::from_secs(3));
        assert_eq!(retry_delay(0, Some(&past), now, 0.5), Some(Duration::ZERO));
        assert_eq!(
            retry_delay(1, Some("soon"), now, 0.0),
            Some(Duration::from_millis(1000))
        );
    }

    #[test]
    fn idempotency_keys_are_v4_uuids() {
        let key = new_idempotency_key();
        let parsed = uuid::Uuid::parse_str(&key).unwrap();
        assert_eq!(parsed.get_version_num(), 4);
        assert_ne!(key, new_idempotency_key());
    }
}
