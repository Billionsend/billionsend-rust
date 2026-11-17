use std::fmt;

/// The details of an error: what the API answered, or what the client made up
/// when there was no answer.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorDetails {
    /// The HTTP status, `None` when no HTTP answer came back (or none was needed,
    /// as for a missing API key or a webhook that does not verify).
    pub status_code: Option<u16>,
    /// The API's error name (`"validation_error"`, `"invalid_api_key"`, …);
    /// `"application_error"` when the API sent none.
    pub name: String,
    /// What went wrong, in words.
    pub message: String,
    /// The answer's `x-request-id`: the id of the request on the dashboard's Logs page.
    pub request_id: Option<String>,
    /// For a 429 or a 5xx, the server's `retry-after` hint in seconds, when it sent one.
    pub retry_after: Option<f64>,
}

impl ErrorDetails {
    pub(crate) fn application(message: impl Into<String>) -> Self {
        Self {
            status_code: None,
            name: APPLICATION_ERROR.to_owned(),
            message: message.into(),
            request_id: None,
            retry_after: None,
        }
    }
}

/// The error name of failures that never got an API answer.
pub const APPLICATION_ERROR: &str = "application_error";

/// Every error the SDK returns. Match on the variant to tell the kinds apart;
/// [`Error::status_code`], [`Error::name`], [`Error::message`] and
/// [`Error::request_id`] read the same fields from any of them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// 400 or 422: the request was refused as invalid.
    Validation(ErrorDetails),
    /// 401, or no API key configured (`missing_api_key`, no status): the key is
    /// missing, malformed or revoked.
    Authentication(ErrorDetails),
    /// 403: the key, its scope, the role or the plan does not allow this.
    PermissionDenied(ErrorDetails),
    /// 404: no such resource or endpoint.
    NotFound(ErrorDetails),
    /// 409: the request conflicts with the current state, or with an earlier use
    /// of its idempotency key.
    Conflict(ErrorDetails),
    /// 429: a rate limit, a sending quota or a stream cap. `retry_after` is the
    /// server's hint in seconds.
    RateLimit(ErrorDetails),
    /// 5xx: the API failed to answer the request. `retry_after` is the
    /// server's hint in seconds, when it sent one.
    Server(ErrorDetails),
    /// Any other answer outside 2xx, or an answer the client could not read.
    Api(ErrorDetails),
    /// The request never got an answer: DNS, connection, TLS or protocol failure.
    Connection(ErrorDetails),
    /// The request timed out before the answer arrived.
    Timeout(ErrorDetails),
    /// A webhook whose signature, timestamp or body did not verify.
    WebhookVerification(ErrorDetails),
    /// An error the client made up without an API answer: a missing path
    /// parameter, a body that does not serialise.
    Application(ErrorDetails),
}

impl Error {
    /// The fields every error carries.
    pub fn details(&self) -> &ErrorDetails {
        match self {
            Error::Validation(d)
            | Error::Authentication(d)
            | Error::PermissionDenied(d)
            | Error::NotFound(d)
            | Error::Conflict(d)
            | Error::RateLimit(d)
            | Error::Server(d)
            | Error::Api(d)
            | Error::Connection(d)
            | Error::Timeout(d)
            | Error::WebhookVerification(d)
            | Error::Application(d) => d,
        }
    }

    /// The HTTP status, `None` without an HTTP answer.
    pub fn status_code(&self) -> Option<u16> {
        self.details().status_code
    }

    /// The API's error name, such as `"validation_error"`; `"application_error"` when the API sent none.
    pub fn name(&self) -> &str {
        &self.details().name
    }

    /// What went wrong, in words.
    pub fn message(&self) -> &str {
        &self.details().message
    }

    /// The answer's `x-request-id`, `None` without one.
    pub fn request_id(&self) -> Option<&str> {
        self.details().request_id.as_deref()
    }

    /// For a 429 or a 5xx, the server's `retry-after` hint in seconds, when it sent one.
    pub fn retry_after(&self) -> Option<f64> {
        self.details().retry_after
    }

    pub(crate) fn application(message: impl Into<String>) -> Self {
        Error::Application(ErrorDetails::application(message))
    }

    pub(crate) fn connection(message: impl Into<String>) -> Self {
        Error::Connection(ErrorDetails::application(message))
    }

    pub(crate) fn timeout(message: impl Into<String>) -> Self {
        Error::Timeout(ErrorDetails::application(message))
    }

    pub(crate) fn webhook(message: impl Into<String>) -> Self {
        Error::WebhookVerification(ErrorDetails {
            name: "webhook_verification_error".to_owned(),
            ..ErrorDetails::application(message)
        })
    }

    /// Maps a non-2xx answer to its variant, keeping the API's own error body
    /// (`statusCode`, `name`, `message`) when it sent one.
    pub(crate) fn from_response(
        status: u16,
        reason: Option<&str>,
        body: &[u8],
        request_id: Option<String>,
        retry_after: Option<&str>,
    ) -> Self {
        let parsed: Option<serde_json::Value> = serde_json::from_slice(body).ok();
        let api = parsed.as_ref().and_then(|v| {
            let name = v.get("name")?.as_str()?;
            let message = v.get("message")?.as_str()?;
            Some((name.to_owned(), message.to_owned()))
        });
        let (name, message) = match api {
            Some(pair) => pair,
            None => {
                let text = String::from_utf8_lossy(body);
                let detail: String = text.trim().chars().take(200).collect();
                let label = match reason {
                    Some(r) if !r.is_empty() => format!("{status} {r}"),
                    _ => status.to_string(),
                };
                let message = if detail.is_empty() {
                    format!("Unexpected {label} response")
                } else {
                    format!("Unexpected {label} response: {detail}")
                };
                (APPLICATION_ERROR.to_owned(), message)
            }
        };
        let hint = || {
            crate::retry::parse_retry_after(retry_after, std::time::SystemTime::now())
                .map(|d| d.as_secs_f64())
        };
        let details = ErrorDetails {
            status_code: Some(status),
            name,
            message,
            request_id,
            retry_after: None,
        };
        match status {
            400 | 422 => Error::Validation(details),
            401 => Error::Authentication(details),
            403 => Error::PermissionDenied(details),
            404 => Error::NotFound(details),
            409 => Error::Conflict(details),
            429 => Error::RateLimit(ErrorDetails {
                retry_after: hint(),
                ..details
            }),
            s if s >= 500 => Error::Server(ErrorDetails {
                retry_after: hint(),
                ..details
            }),
            _ => Error::Api(details),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.details();
        match d.status_code {
            Some(status) => write!(f, "{} ({status} {})", d.message, d.name)?,
            None => write!(f, "{} ({})", d.message, d.name)?,
        }
        if let Some(id) = &d.request_id {
            write!(f, " [request {id}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// `Result` with this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
