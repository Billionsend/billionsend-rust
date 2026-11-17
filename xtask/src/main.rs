//! Repository tasks.
//!
//! ```text
//! cargo xtask sync [--from <path> | --url <url>]   refresh openapi/api.openapi.json
//! cargo xtask generate [--check]                   write (or check) src/generated.rs
//! ```
//!
//! `sync` copies the API's OpenAPI document into the snapshot the SDK is built
//! and tested against. Without arguments it reads the document from a checkout
//! of the API repository next to this one (`../billionsend`, the file
//! `apps/docs/public/api.openapi.json`, as committed on its `main`). After a
//! sync, run `generate` and the tests: whatever fails is what the SDK has to
//! follow.
//!
//! `generate` writes one method per operation, named after its `operationId`
//! (the dotted path of the method, `emails.receiving.attachments.list` being
//! `client.emails().receiving().attachments().list(..)`), and one request
//! builder per operation with a setter for each query parameter. An operation
//! that is also a group of others becomes the group's `get`: `emails.metrics`
//! is `client.emails().metrics().get()`, beside `.links()`, `.countries()` and
//! `.clients()`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde_json::{Map, Value};

const SNAPSHOT: &str = "openapi/api.openapi.json";
const GENERATED: &str = "src/generated.rs";
const SIBLING_REPOSITORY: &str = "../billionsend";
const SIBLING_DOCUMENT: &str = "apps/docs/public/api.openapi.json";
const METHODS: [&str; 5] = ["get", "put", "post", "patch", "delete"];

/// Names of other products that must not appear in this repository: a free-text
/// field naming one is dropped from the snapshot.
const FOREIGN_NAMES: [&str; 2] = ["re\u{73}end", "sv\u{69}x"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("sync") => sync(&args[1..]),
        Some("generate") => generate(args.get(1).map(String::as_str) == Some("--check")),
        _ => {
            Err("usage: cargo xtask sync [--from <path> | --url <url>] | generate [--check]".into())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

// ------------------------------------------------------------------- sync

fn names_foreign_product(text: &str) -> bool {
    let lower = text.to_lowercase();
    FOREIGN_NAMES.iter().any(|name| {
        lower.match_indices(name).any(|(at, _)| {
            let before = lower[..at].chars().next_back();
            let after = lower[at + name.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
    })
}

fn sanitize(node: &mut Value) -> usize {
    match node {
        Value::Object(map) => {
            let before = map.len();
            map.retain(|key, value| {
                !(matches!(key.as_str(), "description" | "summary")
                    && value.as_str().is_some_and(names_foreign_product))
            });
            let mut dropped = before - map.len();
            for value in map.values_mut() {
                dropped += sanitize(value);
            }
            dropped
        }
        Value::Array(items) => items.iter_mut().map(sanitize).sum(),
        _ => 0,
    }
}

fn sync(args: &[String]) -> Result<(), String> {
    let text = match args {
        [flag, path] if flag == "--from" => {
            std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?
        }
        [flag, url] if flag == "--url" => ureq::get(url)
            .header("accept", "application/json")
            .call()
            .map_err(|e| format!("GET {url}: {e}"))?
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("GET {url}: {e}"))?,
        [] => {
            let repository = root().join(SIBLING_REPOSITORY);
            let output = Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(["show", &format!("main:{SIBLING_DOCUMENT}")])
                .output()
                .map_err(|e| format!("git show in {}: {e}", repository.display()))?;
            if !output.status.success() {
                return Err(format!(
                    "git show main:{SIBLING_DOCUMENT} in {} failed: {}",
                    repository.display(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            String::from_utf8(output.stdout).map_err(|e| e.to_string())?
        }
        _ => return Err("usage: cargo xtask sync [--from <path> | --url <url>]".into()),
    };
    let mut document: Value = serde_json::from_str(&text).map_err(|e| format!("not JSON: {e}"))?;
    let version = document["openapi"].as_str().unwrap_or_default();
    if !version.starts_with("3.1") {
        return Err(format!("expected an OpenAPI 3.1 document, got {version:?}"));
    }
    let dropped = sanitize(&mut document);
    let out = root().join(SNAPSHOT);
    let mut json = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?;
    json.push('\n');
    std::fs::write(&out, json).map_err(|e| format!("write {}: {e}", out.display()))?;
    println!(
        "wrote {} ({} operations, {dropped} free-text fields dropped)",
        out.display(),
        operations(&document).len()
    );
    Ok(())
}

// ------------------------------------------------------------------- model

struct Operation<'a> {
    method: &'a str,
    path: &'a str,
    spec: &'a Map<String, Value>,
    id: String,
}

fn operations(document: &Value) -> Vec<Operation<'_>> {
    let mut out = Vec::new();
    for (path, item) in document["paths"].as_object().into_iter().flatten() {
        for method in METHODS {
            if let Some(spec) = item.get(method).and_then(Value::as_object) {
                let id = spec["operationId"]
                    .as_str()
                    .expect("every operation has an operationId")
                    .to_owned();
                out.push(Operation {
                    method,
                    path,
                    spec,
                    id,
                });
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn deref<'a>(document: &'a Value, mut schema: &'a Value) -> &'a Value {
    while let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let pointer = reference.strip_prefix('#').expect("local reference");
        schema = document.pointer(pointer).expect("reference resolves");
    }
    schema
}

fn snake(text: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in text.chars() {
        if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        } else {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            prev_lower = false;
        }
    }
    out.trim_end_matches('_').to_owned()
}

fn pascal(segments: &[String]) -> String {
    segments
        .iter()
        .flat_map(|s| s.split(|c: char| !c.is_ascii_alphanumeric()))
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut chars = s.chars();
            let first = chars.next().expect("non-empty").to_ascii_uppercase();
            std::iter::once(first).chain(chars).collect::<String>()
        })
        .collect()
}

const KEYWORDS: [&str; 39] = [
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

fn ident(name: &str) -> String {
    if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_owned()
    }
}

/// Text from the document as rustdoc lines (markdown special characters escaped).
fn doc_lines(text: &str, indent: &str) -> String {
    let mut out = String::new();
    for line in text.trim().lines() {
        // Markdown outside code spans: brackets and angle brackets are escaped
        // so rustdoc reads them as text, not links or HTML.
        let line: String = line
            .trim()
            .split('`')
            .enumerate()
            .map(|(i, part)| {
                if i % 2 == 1 {
                    part.to_owned()
                } else {
                    part.replace('[', "\\[")
                        .replace(']', "\\]")
                        .replace('<', "\\<")
                        .replace('>', "\\>")
                }
            })
            .collect::<Vec<_>>()
            .join("`");
        if line.is_empty() {
            let _ = writeln!(out, "{indent}///");
        } else {
            let _ = writeln!(out, "{indent}/// {line}");
        }
    }
    out
}

/// Where an operation's method lives: its namespace (resource) and its name.
/// An operation that is also the namespace of others (`emails.metrics`, with
/// `emails.metrics.links` under it) becomes that namespace's `get`.
fn placement(id: &str, ids: &[String]) -> (Vec<String>, String) {
    let segments: Vec<String> = id.split('.').map(str::to_owned).collect();
    let prefix = format!("{id}.");
    if ids.iter().any(|other| other.starts_with(&prefix)) {
        assert!(
            !ids.contains(&format!("{id}.get")),
            "{id} is both an operation and a namespace with its own get"
        );
        return (segments, "get".to_owned());
    }
    let namespace = segments[..segments.len() - 1].to_vec();
    (namespace, snake(&segments[segments.len() - 1]))
}

#[derive(Default)]
struct Namespace {
    children: BTreeMap<String, Namespace>,
    methods: Vec<String>,
}

// ------------------------------------------------------------------- generate

struct QueryParam {
    name: String,
    setter: String,
    kind: &'static str,
    description: String,
}

fn query_params(document: &Value, op: &Operation<'_>) -> Vec<QueryParam> {
    let mut out = Vec::new();
    for parameter in op
        .spec
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let parameter = deref(document, parameter);
        if parameter["in"] != "query" {
            continue;
        }
        let name = parameter["name"]
            .as_str()
            .expect("named parameter")
            .to_owned();
        let schema = deref(document, &parameter["schema"]);
        let types: Vec<&str> = match &schema["type"] {
            Value::String(t) => vec![t.as_str()],
            Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let kind = if types.contains(&"array") {
            "list"
        } else if types.contains(&"integer") {
            "integer"
        } else if types.contains(&"number") {
            "number"
        } else if types.contains(&"boolean") {
            "boolean"
        } else {
            "string"
        };
        let description = parameter["description"]
            .as_str()
            .or_else(|| schema["description"].as_str())
            .unwrap_or_default()
            .to_owned();
        out.push(QueryParam {
            setter: ident(&snake(&name)),
            name,
            kind,
            description,
        });
    }
    out
}

fn path_params(path: &str) -> Vec<String> {
    path.split('{')
        .skip(1)
        .map(|s| s.split('}').next().expect("closed").to_owned())
        .collect()
}

fn success_media(op: &Operation<'_>) -> (&'static str, Option<Value>) {
    let responses = op.spec["responses"].as_object().expect("responses");
    let mut codes: Vec<&String> = responses.keys().filter(|c| c.starts_with('2')).collect();
    codes.sort();
    let response = &responses[codes.first().copied().expect("a success response")];
    let content = response.get("content").and_then(Value::as_object);
    if content.is_some_and(|c| c.contains_key("text/event-stream")) {
        return ("stream", None);
    }
    (
        "json",
        content
            .and_then(|c| c.get("application/json"))
            .and_then(|m| m.get("schema"))
            .cloned(),
    )
}

const RESERVED_SETTERS: [&str; 10] = [
    "header",
    "timeout",
    "max_retries",
    "json",
    "raw",
    "idempotency_key",
    "into_stream",
    "last_event_id",
    "reconnect",
    "batch_validation",
];

fn render(document: &Value) -> String {
    let ops = operations(document);
    let ids: Vec<String> = ops.iter().map(|o| o.id.clone()).collect();
    let mut tree = Namespace::default();
    let mut resources = String::new();
    let mut requests = String::new();
    let mut methods: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();

    for op in &ops {
        let (namespace, method_name) = placement(&op.id, &ids);
        let mut node = &mut tree;
        for segment in &namespace {
            node = node.children.entry(segment.clone()).or_default();
        }
        node.methods.push(method_name.clone());

        let builder = pascal(&op.id.split('.').map(str::to_owned).collect::<Vec<_>>());
        let params = path_params(op.path);
        let query = query_params(document, op);
        let has_body = op.spec.contains_key("requestBody");
        let headers: Vec<String> = op
            .spec
            .get("parameters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|p| deref(document, p))
            .filter(|p| p["in"] == "header")
            .map(|p| p["name"].as_str().expect("named").to_ascii_lowercase())
            .collect();
        let idempotent = headers.iter().any(|h| h == "idempotency-key");
        let batch_validation = headers.iter().any(|h| h == "x-batch-validation");
        let unknown: Vec<&String> = headers
            .iter()
            .filter(|h| *h != "idempotency-key" && *h != "x-batch-validation")
            .collect();
        assert!(
            unknown.is_empty(),
            "{}: unhandled header parameters {unknown:?}",
            op.id
        );
        let no_auth = op
            .spec
            .get("security")
            .is_some_and(|s| s.as_array().is_some_and(Vec::is_empty));
        let (media, response_schema) = success_media(op);
        let stream = media == "stream";
        let paginated = !stream
            && query.iter().any(|q| q.name == "after")
            && response_schema.as_ref().is_some_and(|s| {
                let s = deref(document, s);
                s["properties"].get("has_more").is_some() && s["properties"].get("data").is_some()
            });
        let method_upper = op.method.to_ascii_uppercase();

        // The method on the resource.
        let mut m = String::new();
        let summary = op
            .spec
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !summary.is_empty() {
            m.push_str(&doc_lines(summary, "    "));
            m.push_str("    ///\n");
        }
        let _ = writeln!(m, "    /// `{method_upper} {}`", op.path);
        if let Some(description) = op.spec.get("description").and_then(Value::as_str) {
            m.push_str("    ///\n");
            m.push_str(&doc_lines(description, "    "));
        }
        let mut args = Vec::new();
        let mut generics = Vec::new();
        for p in &params {
            args.push(format!("{}: impl AsRef<str>", ident(&snake(p))));
        }
        if has_body {
            generics.push("B: serde::Serialize + ?Sized".to_owned());
            args.push("body: &B".to_owned());
        }
        let generic = if generics.is_empty() {
            String::new()
        } else {
            format!("<{}>", generics.join(", "))
        };
        let _ = writeln!(
            m,
            "    pub fn {}{generic}(&self{}) -> requests::{builder} {{",
            ident(&method_name),
            args.iter().map(|a| format!(", {a}")).collect::<String>()
        );
        let path_pairs: Vec<String> = params
            .iter()
            .map(|p| format!("(\"{p}\", {}.as_ref())", ident(&snake(p))))
            .collect();
        let _ = write!(
            m,
            "        let inner = Request::new(self.client, Method::{method_upper}, \"{}\", &[{}])",
            op.path,
            path_pairs.join(", ")
        );
        if has_body {
            m.push_str(".json(body)");
        }
        if idempotent {
            m.push_str(".idempotent()");
        }
        if no_auth {
            m.push_str(".without_auth()");
        }
        m.push_str(";\n");
        if stream {
            let _ = writeln!(
                m,
                "        requests::{builder} {{ inner, options: StreamOptions::default() }}"
            );
        } else {
            let _ = writeln!(m, "        requests::{builder} {{ inner }}");
        }
        m.push_str("    }\n");
        methods.entry(namespace.clone()).or_default().push(m);

        // The request builder.
        let mut r = String::new();
        let _ = writeln!(
            r,
            "/// The request of [`{}`](crate::resources::{}::{}): `{method_upper} {}`.",
            op.id,
            if namespace.is_empty() {
                "Root".to_owned()
            } else {
                pascal(&namespace)
            },
            ident(&method_name),
            op.path
        );
        r.push_str("///\n");
        if stream {
            r.push_str(
                "/// Await it to open the stream: an [`EventStream`](crate::sse::EventStream).\n",
            );
            r.push_str(
                "#[derive(Debug, Clone)]\n#[must_use = \"a stream opens only when awaited\"]\n",
            );
            let _ = writeln!(
                r,
                "pub struct {builder} {{\n    pub(crate) inner: Request,\n    pub(crate) options: StreamOptions,\n}}"
            );
        } else {
            r.push_str("/// Await it to send the request; it resolves to the answer's JSON.\n");
            r.push_str(
                "#[derive(Debug, Clone)]\n#[must_use = \"a request is sent only when awaited\"]\n",
            );
            let _ = writeln!(
                r,
                "pub struct {builder} {{\n    pub(crate) inner: Request,\n}}"
            );
        }
        let setters: Vec<&QueryParam> = query
            .iter()
            .filter(|q| !(stream && q.name == "last_event_id"))
            .collect();
        if !setters.is_empty() || batch_validation {
            let _ = writeln!(r, "impl {builder} {{");
            for q in setters {
                assert!(
                    !RESERVED_SETTERS.contains(&q.setter.as_str()),
                    "{}: query parameter {} collides with a builder method",
                    op.id,
                    q.name
                );
                if !q.description.is_empty() {
                    r.push_str(&doc_lines(&q.description, "    "));
                    r.push_str("    ///\n");
                }
                let _ = writeln!(r, "    /// The `{}` query parameter.", q.name);
                let (sig, value) = match q.kind {
                    "list" => (
                        "<I, S>(mut self, values: I) -> Self where I: IntoIterator<Item = S>, S: Into<String>".to_owned(),
                        "join_list(values)".to_owned(),
                    ),
                    "integer" => ("(mut self, value: i64) -> Self".to_owned(), "value.to_string()".to_owned()),
                    "number" => ("(mut self, value: f64) -> Self".to_owned(), "value.to_string()".to_owned()),
                    "boolean" => ("(mut self, value: bool) -> Self".to_owned(), "value.to_string()".to_owned()),
                    _ => ("(mut self, value: impl Into<String>) -> Self".to_owned(), "value.into()".to_owned()),
                };
                let _ = writeln!(
                    r,
                    "    pub fn {}{sig} {{\n        self.inner.set_query(\"{}\", {value});\n        self\n    }}",
                    q.setter, q.name
                );
            }
            if batch_validation {
                r.push_str("    /// The `x-batch-validation` header: `strict` (the default) refuses the whole batch when one item is invalid; `permissive` accepts the valid items and reports the others.\n");
                r.push_str("    pub fn batch_validation(mut self, mode: impl Into<String>) -> Self {\n        self.inner.set_header(\"x-batch-validation\", mode);\n        self\n    }\n");
            }
            r.push_str("}\n");
        }
        if stream {
            let ends = if params.iter().any(|p| p == "inbox_id") {
                "INBOX_GONE_EVENTS"
            } else {
                "&[]"
            };
            let _ = writeln!(r, "stream_builder!({builder}, {ends});");
        } else {
            let _ = writeln!(r, "request_builder!({builder});");
            if idempotent {
                let _ = writeln!(r, "idempotent_builder!({builder});");
            }
            if paginated {
                let _ = writeln!(r, "paginated_builder!({builder});");
            }
        }
        r.push('\n');
        requests.push_str(&r);
    }

    // Resources, depth first.
    fn emit(
        path: &[String],
        node: &Namespace,
        methods: &BTreeMap<Vec<String>, Vec<String>>,
        out: &mut String,
    ) {
        let name = if path.is_empty() {
            "Root".to_owned()
        } else {
            pascal(path)
        };
        if !path.is_empty() {
            let _ = writeln!(out, "/// The `{}` operations.", path.join("."));
            let _ = writeln!(
                out,
                "#[derive(Debug, Clone, Copy)]\npub struct {name}<'a> {{\n    pub(crate) client: &'a BillionSend,\n}}\n"
            );
        }
        let target = if path.is_empty() {
            "BillionSend".to_owned()
        } else {
            format!("{name}<'_>")
        };
        let receiver = if path.is_empty() {
            "self"
        } else {
            "self.client"
        };
        let _ = writeln!(out, "impl {target} {{");
        for (segment, child) in &node.children {
            let mut child_path = path.to_vec();
            child_path.push(segment.clone());
            let child_name = pascal(&child_path);
            let _ = writeln!(out, "    /// The `{}` operations.", child_path.join("."));
            let _ = writeln!(
                out,
                "    pub fn {}(&self) -> {}{child_name}<'_> {{\n        {}{child_name} {{ client: {receiver} }}\n    }}",
                ident(&snake(segment)),
                if path.is_empty() { "resources::" } else { "" },
                if path.is_empty() { "resources::" } else { "" },
            );
            let _ = child;
        }
        if path.is_empty() {
            assert!(
                methods.get(path).is_none_or(Vec::is_empty),
                "operations at the root are not supported"
            );
        }
        for m in methods.get(path).into_iter().flatten() {
            out.push_str(&m.replace(
                "Request::new(self.client,",
                &format!("Request::new({receiver},"),
            ));
        }
        out.push_str("}\n\n");
        for (segment, child) in &node.children {
            let mut child_path = path.to_vec();
            child_path.push(segment.clone());
            emit(&child_path, child, methods, out);
        }
    }
    let mut root_impl = String::new();
    let mut nested = String::new();
    {
        let mut all = String::new();
        emit(&[], &tree, &methods, &mut all);
        // The root impl is the first block; the rest are resource structs.
        let end = all.find("}\n\n").expect("root impl") + 3;
        root_impl.push_str(&all[..end]);
        nested.push_str(&all[end..]);
    }
    resources.push_str(&nested);

    let mut out = String::new();
    out.push_str(
        "// Generated by `cargo xtask generate` from openapi/api.openapi.json. Do not edit.\n\n",
    );
    out.push_str("use crate::client::BillionSend;\n\n");
    out.push_str(&root_impl);
    out.push_str("/// The resources: one struct per group of operations, reached from the client.\npub mod resources {\n");
    out.push_str("#![allow(rustdoc::bare_urls, rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]\n");
    out.push_str("use reqwest::Method;\n\nuse super::requests;\nuse crate::client::BillionSend;\nuse crate::request::Request;\nuse crate::sse::StreamOptions;\n\n");
    out.push_str(&resources);
    out.push_str("}\n\n");
    out.push_str("/// The request builders: one per operation, with a setter for each query parameter.\npub mod requests {\n");
    out.push_str("#![allow(rustdoc::bare_urls, rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]\n");
    out.push_str("use crate::request::{Request, idempotent_builder, join_list, paginated_builder, request_builder};\nuse crate::sse::{INBOX_GONE_EVENTS, StreamOptions, stream_builder};\n\n");
    out.push_str(&requests);
    out.push_str("}\n");
    out
}

fn rustfmt(code: &str) -> Result<String, String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .current_dir(root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("rustfmt: {e}"))?;
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(code.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "rustfmt failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn generate(check: bool) -> Result<(), String> {
    let root = root();
    let text = std::fs::read_to_string(root.join(SNAPSHOT))
        .map_err(|e| format!("read {SNAPSHOT}: {e}"))?;
    let document: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let code = rustfmt(&render(&document))?;
    let target = root.join(GENERATED);
    if check {
        let current = std::fs::read_to_string(&target).unwrap_or_default();
        if current != code {
            return Err(format!(
                "{GENERATED} is out of date: run `cargo xtask generate`"
            ));
        }
        println!("{GENERATED} matches the snapshot");
        return Ok(());
    }
    std::fs::write(&target, code).map_err(|e| format!("write {GENERATED}: {e}"))?;
    println!(
        "wrote {GENERATED} ({} operations)",
        operations(&document).len()
    );
    Ok(())
}
