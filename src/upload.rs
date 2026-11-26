//! Files of the upload operations, sent as `multipart/form-data`.

use bytes::Bytes;

use crate::error::{Error, Result};

/// A file for one of the upload operations: a logo
/// (`brand().logo().upload`, `domains().unsubscribe_page().logo().upload`) or
/// the CSV of a contact import (`contacts().imports().create`).
///
/// Bytes convert into it (`Vec<u8>`, `&[u8]`, [`Bytes`]), so a method takes
/// them as they are. Without a name of its own a logo is sent as `logo.png`,
/// `logo.jpg` or `logo.webp` with the matching content type, judged from its
/// first bytes (the API judges the format from the bytes too), and a CSV as
/// `contacts.csv`, `text/csv`.
///
/// An upload is not repeated after a 5xx, a timeout or a dropped connection;
/// a `429` is retried, since the API answers it before it writes anything.
///
/// ```no_run
/// # async fn run(bs: billionsend::BillionSend) -> Result<(), Box<dyn std::error::Error>> {
/// use billionsend::FileUpload;
///
/// bs.brand().logo().upload(std::fs::read("logo.png")?).await?;
///
/// let csv = FileUpload::new(std::fs::read("people.csv")?).file_name("people.csv");
/// bs.contacts().imports().create(csv).on_conflict("skip").await?;
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct FileUpload {
    bytes: Bytes,
    file_name: Option<String>,
    content_type: Option<String>,
}

impl std::fmt::Debug for FileUpload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileUpload")
            .field("len", &self.bytes.len())
            .field("file_name", &self.file_name)
            .field("content_type", &self.content_type)
            .finish()
    }
}

impl FileUpload {
    /// The file's content.
    pub fn new(bytes: impl Into<Bytes>) -> Self {
        Self {
            bytes: bytes.into(),
            file_name: None,
            content_type: None,
        }
    }

    /// The file name sent with the upload.
    pub fn file_name(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = Some(file_name.into());
        self
    }

    /// The content type sent with the upload.
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }
}

impl From<Bytes> for FileUpload {
    fn from(bytes: Bytes) -> Self {
        Self::new(bytes)
    }
}

impl From<Vec<u8>> for FileUpload {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

impl From<&[u8]> for FileUpload {
    fn from(bytes: &[u8]) -> Self {
        Self::new(Bytes::copy_from_slice(bytes))
    }
}

impl<const N: usize> From<&[u8; N]> for FileUpload {
    fn from(bytes: &[u8; N]) -> Self {
        Self::from(bytes.as_slice())
    }
}

/// What an operation's file is, which decides the name and type it gets by default.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FileKind {
    Csv,
    Image,
}

impl FileKind {
    /// The default file name and content type.
    fn defaults(self, bytes: &[u8]) -> (&'static str, &'static str) {
        match self {
            FileKind::Csv => ("contacts.csv", "text/csv"),
            FileKind::Image if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => ("logo.png", "image/png"),
            FileKind::Image if bytes.starts_with(b"\xFF\xD8\xFF") => ("logo.jpg", "image/jpeg"),
            FileKind::Image
                if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) =>
            {
                ("logo.webp", "image/webp")
            }
            FileKind::Image => ("logo", "application/octet-stream"),
        }
    }
}

/// A name as it goes between the quotes of a part's header: what would end the
/// quotes or the line is percent-encoded.
fn quoted(name: &str) -> String {
    name.replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// The body of an upload: one file and its text fields.
#[derive(Clone, Debug)]
pub(crate) struct Form {
    boundary: String,
    fields: Vec<(&'static str, String)>,
    file_field: &'static str,
    file: FileUpload,
    kind: FileKind,
}

impl Form {
    pub(crate) fn new(file_field: &'static str, file: FileUpload, kind: FileKind) -> Self {
        Self {
            boundary: format!("billionsend-{}", uuid::Uuid::new_v4().simple()),
            fields: Vec::new(),
            file_field,
            file,
            kind,
        }
    }

    pub(crate) fn set_field(&mut self, name: &'static str, value: String) {
        self.fields.retain(|(n, _)| *n != name);
        self.fields.push((name, value));
    }

    pub(crate) fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    pub(crate) fn encode(&self) -> Result<Bytes> {
        let (default_name, default_type) = self.kind.defaults(&self.file.bytes);
        let file_name = self.file.file_name.as_deref().unwrap_or(default_name);
        let content_type = self.file.content_type.as_deref().unwrap_or(default_type);
        if !content_type.bytes().all(|b| (b' '..=b'~').contains(&b)) {
            return Err(Error::invalid_header(
                "Invalid content type for the uploaded file",
            ));
        }
        let boundary = &self.boundary;
        let mut body = Vec::with_capacity(self.file.bytes.len() + 512);
        for (name, value) in &self.fields {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        }
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {content_type}\r\n\r\n",
                self.file_field,
                quoted(file_name)
            )
            .as_bytes(),
        );
        body.extend_from_slice(&self.file.bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        Ok(body.into())
    }
}
