//! A contract mock of the BillionSend API built from the OpenAPI snapshot.
//!
//! It is an HTTP server (wiremock) answering every request. For each one it:
//!
//! 1. finds the operation (method + path template) in the snapshot, or records a violation;
//! 2. checks the API key header (absent only where the operation's security is empty);
//! 3. validates path, query and header parameters and the JSON body against the
//!    operation's schemas (JSON Schema 2020-12, references resolved in the snapshot);
//! 4. answers with a queued response, or else with an instance of the operation's
//!    success schema generated from the schema itself (and validated against it).
//!
//! Every hit is recorded so the coverage test can compare what the SDK called
//! with what the snapshot declares. A test ends with [`ContractMock::assert_clean`].

#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use billionsend::BillionSend;
use serde_json::{Map, Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub const API_KEY: &str = "bs_test_key";
pub const ID: &str = "6a1f1d38-3c55-4a4e-9a57-0f2b3c4d5e6f";
pub const ID2: &str = "7b2e2e49-4d66-4b5f-8b68-1a3c4d5e6f70";
pub const INBOX: &str = "support@example.com";
/// The first bytes of a PNG, and a small CSV, for the uploads.
pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR";
pub const CSV: &[u8] = b"email,first_name\nana@example.com,Ana\n";

pub static SPEC: LazyLock<Value> = LazyLock::new(|| {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/openapi/api.openapi.json"
    ))
    .expect("snapshot");
    serde_json::from_str(&text).expect("snapshot is JSON")
});

const METHODS: [&str; 5] = ["get", "put", "post", "patch", "delete"];

#[derive(Clone, Debug)]
pub struct Operation {
    pub method: String,
    pub path: String,
    pub pointer: String,
    literal_segments: usize,
}

impl Operation {
    pub fn key(&self) -> (String, String) {
        (self.method.clone(), self.path.clone())
    }

    pub fn spec(&self) -> &'static Value {
        SPEC.pointer(&self.pointer).expect("operation")
    }

    fn params(&self, place: &str) -> Vec<(usize, &'static Value)> {
        self.spec()["parameters"]
            .as_array()
            .map(|ps| {
                ps.iter()
                    .enumerate()
                    .filter(|(_, p)| deref(p)["in"] == place)
                    .map(|(i, p)| (i, deref(p)))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn needs_auth(&self) -> bool {
        let security = self.spec().get("security").or_else(|| SPEC.get("security"));
        !security.is_some_and(|s| s.as_array().is_some_and(Vec::is_empty))
    }

    /// The first 2xx answer: status, media type and the pointer of its schema.
    fn success(&self) -> (u16, String, Option<String>) {
        let responses = self.spec()["responses"].as_object().expect("responses");
        let mut codes: Vec<&String> = responses.keys().filter(|c| c.starts_with('2')).collect();
        codes.sort();
        let code = codes[0];
        let content = responses[code].get("content").and_then(Value::as_object);
        let Some((media, body)) = content.and_then(|c| c.iter().next()) else {
            return (code.parse().unwrap(), "application/json".into(), None);
        };
        let pointer = body.get("schema").map(|_| {
            format!(
                "{}/responses/{}/content/{}/schema",
                self.pointer,
                escape(code),
                escape(media)
            )
        });
        (code.parse().unwrap(), media.clone(), pointer)
    }
}

pub fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

pub static OPERATIONS: LazyLock<Vec<Operation>> = LazyLock::new(|| {
    let mut ops = Vec::new();
    for (path, item) in SPEC["paths"].as_object().unwrap() {
        for method in METHODS {
            if item.get(method).is_some() {
                ops.push(Operation {
                    method: method.to_uppercase(),
                    path: path.clone(),
                    pointer: format!("/paths/{}/{method}", escape(path)),
                    literal_segments: path
                        .split('/')
                        .filter(|s| !s.is_empty() && !s.starts_with('{'))
                        .count(),
                });
            }
        }
    }
    // Literal segments win over templated ones (/emails/metrics before /emails/{id}).
    ops.sort_by(|a, b| {
        b.literal_segments
            .cmp(&a.literal_segments)
            .then(a.path.cmp(&b.path))
    });
    ops
});

pub fn operation_keys() -> BTreeSet<(String, String)> {
    OPERATIONS.iter().map(Operation::key).collect()
}

pub fn deref(mut schema: &Value) -> &Value {
    while let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        schema = SPEC
            .pointer(reference.strip_prefix('#').unwrap())
            .expect("reference");
    }
    schema
}

fn match_path(template: &str, path: &str) -> Option<HashMap<String, String>> {
    let t: Vec<&str> = template.split('/').collect();
    let p: Vec<&str> = path.split('/').collect();
    if t.len() != p.len() {
        return None;
    }
    let mut params = HashMap::new();
    for (ts, ps) in t.iter().zip(&p) {
        if let Some(name) = ts.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            if ps.is_empty() {
                return None;
            }
            let decoded = percent_decode(ps);
            params.insert(name.to_owned(), decoded);
        } else if ts != ps {
            return None;
        }
    }
    Some(params)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

// ------------------------------------------------------------------- validation

static VALIDATORS: LazyLock<Mutex<HashMap<String, Arc<jsonschema::Validator>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn validator(pointer: &str) -> Arc<jsonschema::Validator> {
    let mut cache = VALIDATORS.lock().unwrap();
    if let Some(v) = cache.get(pointer) {
        return Arc::clone(v);
    }
    let mut root = SPEC.clone();
    root.as_object_mut().unwrap().insert(
        "$ref".into(),
        Value::String(format!(
            "#{}",
            pointer.replace('{', "%7B").replace('}', "%7D")
        )),
    );
    let compiled = Arc::new(jsonschema::draft202012::new(&root).expect("schema compiles"));
    cache.insert(pointer.to_owned(), Arc::clone(&compiled));
    compiled
}

/// Errors of `instance` against the schema at `pointer` in the snapshot.
pub fn violations(instance: &Value, pointer: &str) -> Vec<String> {
    validator(pointer)
        .iter_errors(instance)
        .take(5)
        .map(|e| format!("{}: {}", e.instance_path(), e))
        .collect()
}

pub fn assert_valid(instance: &Value, pointer: &str, what: &str) {
    let errors = violations(instance, pointer);
    assert!(
        errors.is_empty(),
        "{what} does not match the contract: {}",
        errors.join("; ")
    );
}

// ------------------------------------------------------------------- examples

fn format_example(format: &str) -> Option<&'static str> {
    Some(match format {
        "uuid" => "0b6f5a2e-7c1d-4e8a-9f3b-2d4c6e8a0b1c",
        "date-time" => "2026-01-01T00:00:00.000Z",
        "date" => "2026-01-01",
        "email" => "user@example.com",
        "uri" => "https://example.com/",
        _ => return None,
    })
}

fn constrains(schema: &Value) -> bool {
    ["type", "enum", "anyOf", "oneOf", "allOf", "properties"]
        .iter()
        .any(|k| schema.get(k).is_some())
}

/// A deterministic instance of `schema`: objects get every property when
/// `full`, otherwise only the required ones.
pub fn example(schema: &Value, full: bool) -> Value {
    let s = deref(schema);
    // An `allOf` with one member that constrains anything (the others only
    // describe it) is that member.
    if let Some(members) = s.get("allOf").and_then(Value::as_array) {
        let mut constraining = members.iter().filter(|m| constrains(deref(m)));
        if let (Some(only), None) = (constraining.next(), constraining.next()) {
            return example(only, full);
        }
    }
    if !constrains(s) {
        return json!({});
    }
    if let Some(variants) = s
        .get("anyOf")
        .or_else(|| s.get("oneOf"))
        .and_then(Value::as_array)
    {
        let non_null: Vec<&Value> = variants
            .iter()
            .filter(|v| deref(v)["type"] != "null")
            .collect();
        return example(non_null.first().copied().unwrap_or(&variants[0]), full);
    }
    if let Some(values) = s.get("enum").and_then(Value::as_array) {
        return values
            .iter()
            .find(|v| !v.is_null())
            .cloned()
            .unwrap_or(Value::Null);
    }
    let types: Vec<&str> = match &s["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
        _ if s.get("properties").is_some() => vec!["object"],
        _ => vec![],
    };
    let kind = types
        .iter()
        .copied()
        .find(|t| *t != "null")
        .unwrap_or("null");
    match kind {
        "string" => {
            if let Some(f) = s["format"].as_str().and_then(format_example) {
                return json!(f);
            }
            let min = s["minLength"].as_u64().unwrap_or(0) as usize;
            let max = s["maxLength"].as_u64().unwrap_or(10_000) as usize;
            let mut text = "value".to_owned();
            while text.len() < min {
                text.push('x');
            }
            text.truncate(max);
            json!(text)
        }
        "integer" => {
            let min = s["minimum"].as_i64();
            let max = s["maximum"].as_i64().unwrap_or(1);
            json!(min.unwrap_or(if max >= 1 { 1 } else { 0 }))
        }
        "number" => json!(s["minimum"].as_f64().unwrap_or(1.0)),
        "boolean" => json!(false),
        "null" => Value::Null,
        "array" => {
            let mut count = s["minItems"].as_u64().unwrap_or(1).max(1);
            if let Some(max) = s["maxItems"].as_u64() {
                count = count.min(max);
            }
            let item = example(s.get("items").unwrap_or(&json!({})), full);
            Value::Array((0..count).map(|_| item.clone()).collect())
        }
        "object" => {
            let required: BTreeSet<&str> = s["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let mut out = Map::new();
            for (key, value) in s["properties"].as_object().into_iter().flatten() {
                if full || required.contains(key.as_str()) {
                    out.insert(key.clone(), example(value, full));
                }
            }
            Value::Object(out)
        }
        other => panic!("cannot build an example of type {other}"),
    }
}

/// The example request body of an operation (every property), or `{}`.
pub fn body_example(op: &Operation) -> Value {
    match op
        .spec()
        .pointer("/requestBody/content/application~1json/schema")
    {
        Some(schema) => example(schema, true),
        None => Value::Null,
    }
}

// ------------------------------------------------------------------- multipart

/// One part of a `multipart/form-data` body.
#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

impl Part {
    pub fn text(&self) -> &str {
        std::str::from_utf8(&self.data).expect("a text part")
    }
}

fn position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The parts of a well-formed body, `None` for anything else.
fn parse_multipart(boundary: &str, body: &[u8]) -> Option<Vec<Part>> {
    let delimiter = format!("\r\n--{boundary}");
    let mut rest = body.strip_prefix(format!("--{boundary}").as_bytes())?;
    let mut parts = Vec::new();
    loop {
        if rest == b"--\r\n" {
            return Some(parts);
        }
        rest = rest.strip_prefix(b"\r\n")?;
        let head_end = position(rest, b"\r\n\r\n")?;
        let head = std::str::from_utf8(&rest[..head_end]).ok()?;
        rest = &rest[head_end + 4..];
        let end = position(rest, delimiter.as_bytes())?;
        let mut part = Part {
            name: String::new(),
            file_name: None,
            content_type: None,
            data: rest[..end].to_vec(),
        };
        for line in head.split("\r\n") {
            let (name, value) = line.split_once(": ")?;
            if name.eq_ignore_ascii_case("content-type") {
                part.content_type = Some(value.to_owned());
                continue;
            }
            if !name.eq_ignore_ascii_case("content-disposition") {
                return None;
            }
            let mut pieces = value.split("; ");
            if pieces.next() != Some("form-data") {
                return None;
            }
            for piece in pieces {
                let (key, quoted) = piece.split_once('=')?;
                let text = quoted.strip_prefix('"')?.strip_suffix('"')?.to_owned();
                match key {
                    "name" => part.name = text,
                    "filename" => part.file_name = Some(text),
                    _ => return None,
                }
            }
        }
        parts.push(part);
        rest = &rest[end + delimiter.len()..];
    }
}

/// Checks an upload against the operation's form schema: declared fields once
/// each, one file part for the binary one, text parts that validate.
fn check_form(op: &Operation, content_type: &str, body: &[u8]) -> Result<Vec<Part>, String> {
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .ok_or("body must be multipart/form-data with a boundary")?;
    let parts = parse_multipart(boundary, body).ok_or("malformed multipart body")?;
    let pointer = format!(
        "{}/requestBody/content/multipart~1form-data/schema",
        op.pointer
    );
    let schema = deref(SPEC.pointer(&pointer).expect("form schema"));
    let mut fields = Map::new();
    let mut files = 0;
    for part in &parts {
        let property = schema["properties"]
            .get(&part.name)
            .ok_or_else(|| format!("undeclared form field {}", part.name))?;
        let binary = deref(property)["format"] == "binary";
        if binary != part.file_name.is_some() || binary != part.content_type.is_some() {
            return Err(format!("field {} is sent as the wrong kind", part.name));
        }
        files += usize::from(binary);
        let value = if binary { "" } else { part.text() };
        if fields.insert(part.name.clone(), json!(value)).is_some() {
            return Err(format!("form field {} sent more than once", part.name));
        }
    }
    if files != 1 {
        return Err(format!("expected one file, got {files}"));
    }
    let errors = violations(&Value::Object(fields), &pointer);
    if !errors.is_empty() {
        return Err(format!("form: {}", errors.join("; ")));
    }
    Ok(parts)
}

// ------------------------------------------------------------------- the mock

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub path_params: HashMap<String, String>,
    pub query: Vec<(String, String)>,
    pub headers: http::HeaderMap,
    pub body: Value,
    /// The parts of an upload.
    pub parts: Vec<Part>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Default)]
struct State {
    calls: Vec<Recorded>,
    queued: HashMap<(String, String), VecDeque<ResponseTemplate>>,
    violations: Vec<String>,
    minimal: bool,
}

#[derive(Clone)]
struct Responder(Arc<Mutex<State>>);

impl Respond for Responder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        match handle(&mut state, request) {
            Ok(answer) => answer,
            Err(violation) => {
                state.violations.push(violation.clone());
                ResponseTemplate::new(599).set_body_json(json!({
                    "statusCode": 599, "name": "contract_violation", "message": violation
                }))
            }
        }
    }
}

fn coerce(values: &str, schema: &Value) -> Value {
    let s = deref(schema);
    let types: Vec<&str> = match &s["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    if types.contains(&"array") {
        return Value::Array(
            values
                .split(',')
                .map(|v| scalar(v, deref(&s["items"])))
                .collect(),
        );
    }
    scalar(values, s)
}

fn scalar(value: &str, s: &Value) -> Value {
    let types: Vec<&str> = match &s["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    if types.contains(&"integer") {
        if let Ok(n) = value.parse::<i64>() {
            return json!(n);
        }
    }
    if types.contains(&"number") {
        if let Ok(n) = value.parse::<f64>() {
            return json!(n);
        }
    }
    if types.contains(&"boolean") && (value == "true" || value == "false") {
        return json!(value == "true");
    }
    json!(value)
}

fn handle(state: &mut State, request: &Request) -> Result<ResponseTemplate, String> {
    let method = request.method.as_str().to_owned();
    let raw_path = request.url.path().to_owned();
    let mut path_matched = false;
    let mut found = None;
    for op in OPERATIONS.iter() {
        if let Some(params) = match_path(&op.path, &raw_path) {
            path_matched = true;
            if op.method == method {
                found = Some((op.clone(), params));
                break;
            }
        }
    }
    let Some((op, path_params)) = found else {
        let reason = if path_matched {
            "method not allowed"
        } else {
            "no such path"
        };
        return Err(format!(
            "{method} {raw_path}: not an operation of the contract ({reason})"
        ));
    };
    let at = format!("{} {}", op.method, op.path);

    // Authentication
    let auth = request
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok());
    if op.needs_auth() {
        if auth != Some(&format!("Bearer {API_KEY}")) {
            return Err(format!("{at}: missing or wrong Authorization header"));
        }
    } else if auth.is_some() {
        return Err(format!("{at}: must be sent without an API key"));
    }

    // Path parameters
    for (i, p) in op.params("path") {
        let name = p["name"].as_str().unwrap();
        let value = path_params
            .get(name)
            .ok_or_else(|| format!("{at}: path parameter {name} missing"))?;
        let errors = violations(
            &json!(value),
            &format!("{}/parameters/{i}/schema", op.pointer),
        );
        if !errors.is_empty() {
            return Err(format!(
                "{at}: path parameter {name}: {}",
                errors.join("; ")
            ));
        }
    }

    // Query parameters
    let query: Vec<(String, String)> = request
        .url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let declared = op.params("query");
    for (name, _) in &query {
        if !declared.iter().any(|(_, p)| p["name"] == name.as_str()) {
            return Err(format!("{at}: undeclared query parameter {name}"));
        }
        if query.iter().filter(|(k, _)| k == name).count() > 1 {
            return Err(format!("{at}: query parameter {name} sent more than once"));
        }
    }
    for (i, p) in &declared {
        let name = p["name"].as_str().unwrap();
        match query.iter().find(|(k, _)| k == name) {
            None if p["required"] == true => {
                return Err(format!("{at}: required query parameter {name} missing"));
            }
            None => {}
            Some((_, value)) => {
                let instance = coerce(value, &p["schema"]);
                let errors =
                    violations(&instance, &format!("{}/parameters/{i}/schema", op.pointer));
                if !errors.is_empty() {
                    return Err(format!("{at}: query {name}: {}", errors.join("; ")));
                }
            }
        }
    }

    // Header parameters
    for (i, p) in op.params("header") {
        let name = p["name"].as_str().unwrap();
        match request.headers.get(name).and_then(|v| v.to_str().ok()) {
            None if p["required"] == true => {
                return Err(format!("{at}: required header {name} missing"));
            }
            None => {}
            Some(value) => {
                let errors = violations(
                    &json!(value),
                    &format!("{}/parameters/{i}/schema", op.pointer),
                );
                if !errors.is_empty() {
                    return Err(format!("{at}: header {name}: {}", errors.join("; ")));
                }
            }
        }
    }

    // Body
    let mut body = Value::Null;
    let mut parts = Vec::new();
    match op.spec().get("requestBody") {
        None if !request.body.is_empty() => return Err(format!("{at}: takes no request body")),
        None => {}
        Some(spec) if request.body.is_empty() => {
            if spec["required"] == true {
                return Err(format!("{at}: request body missing"));
            }
        }
        Some(spec) if spec["content"].get("multipart/form-data").is_some() => {
            let content_type = request
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            parts =
                check_form(&op, content_type, &request.body).map_err(|e| format!("{at}: {e}"))?;
        }
        Some(_) => {
            let content_type = request
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !content_type.starts_with("application/json") {
                return Err(format!("{at}: body must be JSON"));
            }
            body = serde_json::from_slice(&request.body)
                .map_err(|e| format!("{at}: body is not JSON: {e}"))?;
            let errors = violations(
                &body,
                &format!(
                    "{}/requestBody/content/application~1json/schema",
                    op.pointer
                ),
            );
            if !errors.is_empty() {
                return Err(format!("{at}: request body: {}", errors.join("; ")));
            }
        }
    }

    state.calls.push(Recorded {
        method: op.method.clone(),
        path: op.path.clone(),
        path_params,
        query,
        headers: request.headers.clone(),
        body,
        parts,
    });

    if let Some(answer) = state
        .queued
        .get_mut(&op.key())
        .and_then(VecDeque::pop_front)
    {
        return Ok(answer);
    }
    let (status, media, pointer) = op.success();
    if media == "text/event-stream" {
        let payload =
            "id: 1\nevent: inbox.email.received\ndata: {\"type\":\"inbox.email.received\"}\n\n";
        return Ok(ResponseTemplate::new(status).set_body_raw(payload, "text/event-stream"));
    }
    let Some(pointer) = pointer else {
        return Ok(ResponseTemplate::new(status).set_body_json(json!({})));
    };
    let instance = example(SPEC.pointer(&pointer).unwrap(), !state.minimal);
    let errors = violations(&instance, &pointer);
    if !errors.is_empty() {
        return Err(format!(
            "{at}: generated answer is invalid: {}",
            errors.join("; ")
        ));
    }
    Ok(ResponseTemplate::new(status).set_body_json(instance))
}

pub struct ContractMock {
    pub server: MockServer,
    state: Arc<Mutex<State>>,
}

impl ContractMock {
    pub async fn start() -> Self {
        let server = MockServer::start().await;
        let state = Arc::new(Mutex::new(State::default()));
        Mock::given(any())
            .respond_with(Responder(Arc::clone(&state)))
            .mount(&server)
            .await;
        Self { server, state }
    }

    pub fn uri(&self) -> String {
        self.server.uri()
    }

    /// A client on this mock with the test key and no retry delays to speak of.
    pub fn client(&self) -> BillionSend {
        self.builder().build().unwrap()
    }

    pub fn builder(&self) -> billionsend::ClientBuilder {
        BillionSend::builder().api_key(API_KEY).base_url(self.uri())
    }

    /// Generated answers carry only the required properties.
    pub fn minimal(&self, minimal: bool) {
        self.state.lock().unwrap().minimal = minimal;
    }

    fn op_key(method: &str, path: &str) -> (String, String) {
        let key = (method.to_owned(), path.to_owned());
        assert!(
            operation_keys().contains(&key),
            "{method} {path} is not in the contract"
        );
        key
    }

    /// Queues one answer for an operation (FIFO), instead of the generated example.
    pub fn respond(&self, method: &str, path: &str, answer: ResponseTemplate) {
        let key = Self::op_key(method, path);
        self.state
            .lock()
            .unwrap()
            .queued
            .entry(key)
            .or_default()
            .push_back(answer);
    }

    /// Queues a JSON answer, checked against the contract first.
    pub fn respond_json(&self, method: &str, path: &str, status: u16, body: Value) {
        let key = Self::op_key(method, path);
        let op = OPERATIONS.iter().find(|o| o.key() == key).unwrap();
        if status >= 400 {
            assert_valid(&body, "/components/schemas/ErrorResponse", "queued error");
        } else if let (_, _, Some(pointer)) = op.success() {
            assert_valid(&body, &pointer, "queued answer");
        }
        self.respond(
            method,
            path,
            ResponseTemplate::new(status).set_body_json(body),
        );
    }

    /// Queues an API error answer.
    pub fn error(
        &self,
        method: &str,
        path: &str,
        status: u16,
        name: &str,
        message: &str,
    ) -> ErrorAnswer<'_> {
        ErrorAnswer {
            mock: self,
            method: method.into(),
            path: path.into(),
            status,
            name: name.into(),
            message: message.into(),
            headers: Vec::new(),
        }
    }

    /// Queues an answer that arrives after `delay` (to make the client time out).
    pub fn delay(&self, method: &str, path: &str, delay: Duration) {
        self.respond(
            method,
            path,
            ResponseTemplate::new(200)
                .set_body_json(json!({}))
                .set_delay(delay),
        );
    }

    pub fn calls(&self) -> Vec<Recorded> {
        self.state.lock().unwrap().calls.clone()
    }

    pub fn last(&self) -> Recorded {
        self.calls().pop().expect("no request was sent")
    }

    pub fn hits(&self) -> BTreeSet<(String, String)> {
        self.calls()
            .iter()
            .map(|c| (c.method.clone(), c.path.clone()))
            .collect()
    }

    pub fn clear(&self) {
        self.state.lock().unwrap().calls.clear();
    }

    /// The violations recorded so far, cleared.
    pub fn take_violations(&self) -> Vec<String> {
        std::mem::take(&mut self.state.lock().unwrap().violations)
    }

    /// Fails when any request broke the contract.
    pub fn assert_clean(&self) {
        let violations = self.state.lock().unwrap().violations.clone();
        assert!(
            violations.is_empty(),
            "contract violations:\n{}",
            violations.join("\n")
        );
    }
}

pub struct ErrorAnswer<'a> {
    mock: &'a ContractMock,
    method: String,
    path: String,
    status: u16,
    name: String,
    message: String,
    headers: Vec<(String, String)>,
}

impl ErrorAnswer<'_> {
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

impl Drop for ErrorAnswer<'_> {
    fn drop(&mut self) {
        let body = json!({"statusCode": self.status, "name": self.name, "message": self.message});
        assert_valid(&body, "/components/schemas/ErrorResponse", "queued error");
        let mut answer = ResponseTemplate::new(self.status).set_body_json(body);
        for (name, value) in &self.headers {
            answer = answer.insert_header(name.as_str(), value.as_str());
        }
        self.mock.respond(&self.method, &self.path, answer);
    }
}
