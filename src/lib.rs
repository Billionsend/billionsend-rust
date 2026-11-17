#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod client;
mod error;
mod generated;
pub mod pagination;
mod request;
pub mod retry;
pub mod sse;
pub mod webhooks;

pub use client::{
    BillionSend, ClientBuilder, DEFAULT_BASE_URL, DEFAULT_MAX_RETRIES, DEFAULT_TIMEOUT,
};
pub use error::{APPLICATION_ERROR, Error, ErrorDetails, Result};
pub use generated::{requests, resources};
pub use request::ApiResponse;
pub use sse::{EventStream, InboxStreamEvent, InboxStreamItem, StreamGapEvent};
pub use webhooks::{WebhookVerifier, verify_webhook};

/// This crate's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
