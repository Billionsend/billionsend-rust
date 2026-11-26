//! Uploads: the multipart body, the defaults of its file, and what is retried.

mod common;

use std::io::Read;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use billionsend::{BillionSend, Error, FileUpload};
use common::{API_KEY, CSV, ContractMock, ID, PNG, Part};
use serde::Serialize;
use serde_json::json;

const JPEG: &[u8] = b"\xFF\xD8\xFF\xE0\x00\x10JFIF";
const WEBP: &[u8] = b"RIFF\x1a\x00\x00\x00WEBPVP8 ";

fn file_of(parts: &[Part]) -> &Part {
    parts
        .iter()
        .find(|p| p.name == "file")
        .expect("a file part")
}

fn field<'a>(parts: &'a [Part], name: &str) -> &'a str {
    parts
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("no {name} field"))
        .text()
}

#[tokio::test]
async fn a_logo_is_named_and_typed_by_its_bytes() {
    let mock = ContractMock::start().await;
    let client = mock.client();
    for (bytes, name, content_type) in [
        (PNG, "logo.png", "image/png"),
        (JPEG, "logo.jpg", "image/jpeg"),
        (WEBP, "logo.webp", "image/webp"),
        (b"<svg/>".as_slice(), "logo", "application/octet-stream"),
    ] {
        client.brand().logo().upload(bytes).await.unwrap();
        let call = mock.last();
        assert_eq!(
            (call.method.as_str(), call.path.as_str()),
            ("PUT", "/brand/logo")
        );
        assert_eq!(call.parts.len(), 1);
        let file = file_of(&call.parts);
        assert_eq!(file.file_name.as_deref(), Some(name));
        assert_eq!(file.content_type.as_deref(), Some(content_type));
        assert_eq!(file.data, bytes, "the bytes arrive as they are");
        assert!(
            call.header("content-type")
                .unwrap()
                .starts_with("multipart/form-data; boundary=billionsend-")
        );
        assert_eq!(call.header("idempotency-key"), None);
    }
    mock.assert_clean();
}

#[tokio::test]
async fn a_domain_logo_with_its_own_name_and_type() {
    let mock = ContractMock::start().await;
    let file = FileUpload::new(PNG.to_vec())
        .file_name("ac\"me\r\nlogo é.png")
        .content_type("image/x-png");
    let answer = mock
        .client()
        .domains()
        .unsubscribe_page()
        .logo()
        .upload(ID, file)
        .await
        .unwrap();
    assert!(answer["logo_url"].is_string());
    let call = mock.last();
    assert_eq!(call.path_params["id"], ID);
    let file = file_of(&call.parts);
    assert_eq!(
        file.file_name.as_deref(),
        Some("ac%22me%0D%0Alogo é.png"),
        "a name cannot end its quotes or its line"
    );
    assert_eq!(file.content_type.as_deref(), Some("image/x-png"));
    mock.assert_clean();
}

#[tokio::test]
async fn a_content_type_that_could_end_its_line_is_refused() {
    let mock = ContractMock::start().await;
    let file = FileUpload::new(PNG.to_vec()).content_type("image/png\r\nx-other: 1");
    let error = mock.client().brand().logo().upload(file).await.unwrap_err();
    assert_eq!(error.name(), "invalid_header");
    assert!(mock.calls().is_empty());
}

#[tokio::test]
async fn a_contact_import_sends_its_fields() {
    #[derive(Serialize)]
    struct ColumnMap {
        email: &'static str,
        first_name: &'static str,
    }
    let mock = ContractMock::start().await;
    mock.client()
        .contacts()
        .imports()
        .create(CSV)
        .column_map(&ColumnMap {
            email: "Email",
            first_name: "First Name",
        })
        .on_conflict("upsert")
        .on_conflict("skip")
        .segments(&json!([{ "id": ID }]))
        .topics(r#"[{"id":"t","subscription":"opt_in"}]"#)
        .await
        .unwrap();
    let call = mock.last();
    let file = file_of(&call.parts);
    assert_eq!(file.file_name.as_deref(), Some("contacts.csv"));
    assert_eq!(file.content_type.as_deref(), Some("text/csv"));
    assert_eq!(file.data, CSV);
    assert_eq!(call.parts.len(), 5, "a field set twice is sent once");
    assert_eq!(
        field(&call.parts, "column_map"),
        r#"{"email":"Email","first_name":"First Name"}"#
    );
    assert_eq!(field(&call.parts, "on_conflict"), "skip");
    assert_eq!(
        field(&call.parts, "segments"),
        format!(r#"[{{"id":"{ID}"}}]"#)
    );
    assert_eq!(
        field(&call.parts, "topics"),
        r#"[{"id":"t","subscription":"opt_in"}]"#,
        "a string is already encoded"
    );

    mock.client()
        .contacts()
        .imports()
        .create(FileUpload::new(CSV.to_vec()).file_name("people.csv"))
        .await
        .unwrap();
    let call = mock.last();
    assert_eq!(call.parts.len(), 1);
    assert_eq!(
        file_of(&call.parts).file_name.as_deref(),
        Some("people.csv")
    );
    mock.assert_clean();
}

#[tokio::test]
async fn a_field_that_does_not_serialise_sends_nothing() {
    let mock = ContractMock::start().await;
    let map = std::collections::HashMap::from([((1, 2), "x")]);
    let error = mock
        .client()
        .contacts()
        .imports()
        .create(CSV)
        .column_map(&map)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Application(_)), "{error:?}");
    assert!(mock.calls().is_empty());
}

#[tokio::test]
async fn an_upload_is_not_repeated_after_a_5xx() {
    let mock = ContractMock::start().await;
    mock.error("PUT", "/brand/logo", 500, "internal_server_error", "Boom")
        .header("retry-after", "0");
    mock.error(
        "POST",
        "/contacts/imports",
        503,
        "internal_server_error",
        "Down",
    )
    .header("retry-after", "0");
    let client = mock.client();
    let error = client.brand().logo().upload(PNG).await.unwrap_err();
    assert!(matches!(error, Error::Server(_)));
    let error = client.contacts().imports().create(CSV).await.unwrap_err();
    assert!(matches!(error, Error::Server(_)));
    assert_eq!(mock.calls().len(), 2);
}

#[tokio::test]
async fn an_upload_is_not_repeated_after_a_timeout() {
    let mock = ContractMock::start().await;
    mock.delay("PUT", "/brand/logo", Duration::from_millis(500));
    let error = mock
        .client()
        .brand()
        .logo()
        .upload(PNG)
        .timeout(Duration::from_millis(50))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Timeout(_)), "{error:?}");
    assert_eq!(mock.calls().len(), 1);
}

#[tokio::test]
async fn an_upload_is_not_repeated_after_a_dropped_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&connections);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
            // Reads the start of the request, then hangs up without an answer.
            let _ = stream.unwrap().read(&mut [0; 64]);
        }
    });
    let client = BillionSend::builder()
        .api_key(API_KEY)
        .base_url(format!("http://{address}"))
        .max_retries(1)
        .build()
        .unwrap();
    let error = client.brand().logo().upload(PNG).await.unwrap_err();
    assert!(matches!(error, Error::Connection(_)), "{error:?}");
    assert_eq!(connections.load(Ordering::SeqCst), 1);
    // The same failure repeats a PUT that is no upload.
    let error = client
        .inboxes()
        .threads()
        .labels()
        .set(ID, ID, &json!({ "label_ids": [] }))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Connection(_)), "{error:?}");
    assert_eq!(connections.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn an_upload_is_repeated_after_a_429_with_the_same_body() {
    let mock = ContractMock::start().await;
    for (method, path) in [("PUT", "/brand/logo"), ("POST", "/contacts/imports")] {
        mock.error(method, path, 429, "rate_limit_exceeded", "Slow down")
            .header("retry-after", "0");
    }
    let client = mock.client();
    client.brand().logo().upload(PNG).await.unwrap();
    client
        .contacts()
        .imports()
        .create(CSV)
        .on_conflict("skip")
        .await
        .unwrap();
    let calls = mock.calls();
    assert_eq!(calls.len(), 4);
    for pair in calls.chunks(2) {
        assert_eq!(
            pair[0].header("content-type"),
            pair[1].header("content-type")
        );
        assert_eq!(pair[0].parts.len(), pair[1].parts.len());
        assert_eq!(file_of(&pair[0].parts).data, file_of(&pair[1].parts).data);
    }
    mock.assert_clean();
}
