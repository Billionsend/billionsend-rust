//! Webhook signature verification, against the API's signing vectors.
//!
//! The first vector is the one the API's own signing test pins (the published
//! webhook signing vector); the rotation vector was signed the way the API
//! signs, with two secrets.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use billionsend::{Error, WebhookVerifier, verify_webhook};
use serde_json::json;

const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
const MSG_ID: &str = "msg_p5jXN8AQM9LWM0D4loKWxJek";
const TIMESTAMP: u64 = 1614265330;
const PAYLOAD: &str = r#"{"test": 2432232314}"#;
const SIGNATURE: &str = "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=";

const ROTATED: &str = "whsec_BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
const ROTATION_PAYLOAD: &str = r#"{"type":"email.delivered","created_at":"2026-11-16T09:00:00.000Z","data":{"email_id":"e1"}}"#;
const ROTATION_SIGNATURE: &str = "v1,VfgCmhkRJMAi9nv9R1rsE49fWcTpLQ8gcMN9e7uGbrk= v1,IRvb7GZmh00vSNTRcSQ0MTb0qfbEKpRQlNS8r3daFnQ=";

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

fn headers() -> [(&'static str, String); 3] {
    [
        ("webhook-id", MSG_ID.to_owned()),
        ("webhook-timestamp", TIMESTAMP.to_string()),
        ("webhook-signature", SIGNATURE.to_owned()),
    ]
}

fn verifier(secret: &str, now: u64) -> WebhookVerifier {
    WebhookVerifier::new(secret).unwrap().now(at(now))
}

#[test]
fn the_published_vector() {
    let event = verifier(SECRET, TIMESTAMP)
        .verify(PAYLOAD, &headers())
        .unwrap();
    assert_eq!(event, json!({ "test": 2432232314u64 }));
}

#[test]
fn bytes_any_header_case_and_header_maps() {
    let upper: Vec<(String, String)> = headers()
        .iter()
        .map(|(k, v)| (k.to_uppercase(), v.clone()))
        .collect();
    assert!(
        verifier(SECRET, TIMESTAMP)
            .verify(PAYLOAD.as_bytes(), &upper)
            .is_ok()
    );

    let map: HashMap<String, String> = headers()
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    assert!(verifier(SECRET, TIMESTAMP).verify(PAYLOAD, &map).is_ok());

    let mut header_map = reqwest::header::HeaderMap::new();
    for (k, v) in headers() {
        header_map.insert(k, v.parse().unwrap());
    }
    assert!(
        verifier(SECRET, TIMESTAMP)
            .verify(PAYLOAD, &header_map)
            .is_ok()
    );
}

#[test]
fn either_secret_verifies_during_a_rotation() {
    let headers = [
        ("webhook-id", "msg_rot"),
        ("webhook-timestamp", "1700000000"),
        ("webhook-signature", ROTATION_SIGNATURE),
    ];
    for secret in [ROTATED, SECRET] {
        let event = verifier(secret, 1_700_000_000)
            .verify(ROTATION_PAYLOAD, &headers)
            .unwrap();
        assert_eq!(event["type"], "email.delivered");
        assert_eq!(event["data"], json!({ "email_id": "e1" }));
    }
}

#[test]
fn a_tampered_body_fails() {
    let error = verifier(SECRET, TIMESTAMP)
        .verify(format!("{PAYLOAD} "), &headers())
        .unwrap_err();
    assert!(matches!(error, Error::WebhookVerification(_)));
    assert_eq!(error.message(), "No webhook signature matches");
    assert_eq!(error.name(), "webhook_verification_error");
    assert_eq!(error.status_code(), None);
}

#[test]
fn another_secret_fails() {
    assert!(
        verifier(ROTATED, TIMESTAMP)
            .verify(PAYLOAD, &headers())
            .is_err()
    );
}

#[test]
fn a_timestamp_outside_the_tolerance_fails() {
    let old = verifier(SECRET, TIMESTAMP + 301)
        .verify(PAYLOAD, &headers())
        .unwrap_err();
    assert_eq!(old.message(), "The webhook timestamp is too old");
    let new = verifier(SECRET, TIMESTAMP - 301)
        .verify(PAYLOAD, &headers())
        .unwrap_err();
    assert_eq!(new.message(), "The webhook timestamp is too new");
    assert!(
        verifier(SECRET, TIMESTAMP + 300)
            .verify(PAYLOAD, &headers())
            .is_ok()
    );
    let wide = verifier(SECRET, TIMESTAMP + 3600).tolerance(Duration::from_secs(3600));
    assert!(wide.verify(PAYLOAD, &headers()).is_ok());
}

#[test]
fn the_default_clock_rejects_an_old_vector() {
    let error = verify_webhook(PAYLOAD, &headers(), SECRET).unwrap_err();
    assert_eq!(error.message(), "The webhook timestamp is too old");
}

#[test]
fn missing_or_malformed_headers_fail() {
    let [id, timestamp, _] = headers();
    let error = verifier(SECRET, TIMESTAMP)
        .verify(PAYLOAD, &[id.clone(), timestamp])
        .unwrap_err();
    assert_eq!(
        error.message(),
        "Missing the webhook-id, webhook-timestamp or webhook-signature header"
    );
    let bad = [
        id,
        ("webhook-timestamp", "12a".to_owned()),
        ("webhook-signature", SIGNATURE.to_owned()),
    ];
    assert_eq!(
        verifier(SECRET, TIMESTAMP)
            .verify(PAYLOAD, &bad)
            .unwrap_err()
            .message(),
        "Invalid webhook-timestamp header"
    );
}

#[test]
fn other_signature_versions_do_not_count() {
    let mut h = headers();
    h[2].1 = SIGNATURE.replacen("v1,", "v2,", 1);
    assert!(verifier(SECRET, TIMESTAMP).verify(PAYLOAD, &h).is_err());
    h[2].1 = format!("garbage v1,!!! {SIGNATURE}");
    assert!(
        verifier(SECRET, TIMESTAMP).verify(PAYLOAD, &h).is_ok(),
        "any matching signature is enough"
    );
}

#[test]
fn malformed_secrets_are_refused() {
    for secret in [
        "MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw",
        "whsec_not base64",
        "whsec_AAAA",
        "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw==",
    ] {
        let error = WebhookVerifier::new(secret).unwrap_err();
        assert_eq!(
            error.message(),
            "The webhook secret is not a whsec_ secret",
            "{secret}"
        );
    }
}

#[test]
fn a_body_that_is_not_json_fails_after_the_signature() {
    use base64::Engine as _;
    use hmac::{KeyInit, Mac};
    let key = base64::engine::general_purpose::STANDARD
        .decode(&SECRET[6..])
        .unwrap();
    let mut mac = <hmac::Hmac<sha2::Sha256> as KeyInit>::new_from_slice(&key).unwrap();
    mac.update(format!("{MSG_ID}.{TIMESTAMP}.not json").as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    let mut h = headers();
    h[2].1 = format!("v1,{signature}");
    let error = verifier(SECRET, TIMESTAMP)
        .verify("not json", &h)
        .unwrap_err();
    assert_eq!(error.message(), "The webhook body is not JSON");
}
