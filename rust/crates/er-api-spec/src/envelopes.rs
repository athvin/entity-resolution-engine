//! Success-envelope classification and deterministic ledger rendering.

use crate::{get_routes, struct_fields};
use std::collections::BTreeMap;

/// How a successful list/detail response is shaped.
#[derive(Debug, PartialEq, Eq)]
pub enum Envelope {
    /// The value serialized directly, with **no wrapper key**:
    /// `Json(json!(rows))`. Deliberately not called "array": the source cannot
    /// tell a `Vec` from a single record here — `Json(json!(out))` in
    /// `list_orgs` is a list and `Json(json!(job))` in `get_job` is one record,
    /// and nothing in the response construction distinguishes them. What this
    /// variant does claim is the part W0.5b turned on: there is no envelope to
    /// unwrap. Whether the payload is a list follows from the resource, which
    /// the consumer already knows.
    Bare,
    /// A JSON object literal or resolved local struct, carrying **all** its top-level keys. The key
    /// count is what separates the two cases the first draft of this test
    /// conflated: one key is a *wrapper* around a payload
    /// (`{"jobs": […]}`), several keys is a *record* built inline
    /// (`{"id": …, "name": …, "state": …}` in `get_org`). Reporting only the
    /// first key called the second one a wrapper named `id`, which is not a
    /// thing any consumer should be told.
    Object(Vec<String>),
    /// The shared keyset-page helper: `{items, next_cursor}`.
    Page,
    /// A JSON value whose shape is unresolved: `Json(typed)`.
    Scalar,
    /// Not JSON at all: an Arrow IPC stream, whose snapshot and next cursor
    /// travel in response **headers** rather than in an envelope. A consumer
    /// that reaches for `body.items` or `body.next_cursor` here finds neither.
    Arrow,
}

impl Envelope {
    pub fn as_json(&self) -> String {
        match self {
            Self::Bare => "{ \"envelope\": \"bare\" }".to_string(),
            Self::Object(keys) => match keys.as_slice() {
                [one] => format!("{{ \"envelope\": \"wrapped\", \"key\": \"{one}\" }}"),
                many => format!(
                    "{{ \"envelope\": \"record\", \"keys\": [{}] }}",
                    many.iter()
                        .map(|k| format!("\"{k}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
            Self::Page => "{ \"envelope\": \"page\", \"key\": \"items\" }".to_string(),
            Self::Scalar => "{ \"envelope\": \"scalar\" }".to_string(),
            Self::Arrow => "{ \"envelope\": \"arrow\", \"cursor_in\": \"headers\" }".to_string(),
        }
    }
}

/// The body of `async fn <name>`, by balanced braces.
pub fn handler_bodies(src: &str) -> BTreeMap<String, String> {
    const MARKER: &str = "async fn ";
    let bytes = src.as_bytes();
    let mut out = BTreeMap::new();
    let mut i = 0usize;
    while let Some(rel) = src[i..].find(MARKER) {
        let at = i + rel;
        let name_start = at + MARKER.len();
        let Some(paren) = src[name_start..].find('(') else {
            break;
        };
        let name = src[name_start..name_start + paren].trim().to_string();
        let Some(brace_rel) = src[name_start + paren..].find('{') else {
            break;
        };
        let open = name_start + paren + brace_rel;
        let mut depth = 1usize;
        let mut j = open + 1;
        while j < bytes.len() && depth > 0 {
            match bytes[j] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        if name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            && !name.is_empty()
        {
            out.insert(name, src[open..j].to_string());
        }
        i = j.max(name_start);
    }
    out
}

/// Strip the error arms, so an error envelope is never mistaken for the success
/// one. `detail_err(...)` and `pg_err(...)` build `{"detail": …}`, which would
/// otherwise classify every handler as `Object("detail")`.
///
/// Each call is removed by **balanced parens**, not by dropping its line: a
/// handler whose match arms share one line would otherwise lose its success arm
/// along with its error arm and be reported unclassifiable. The unit test below
/// pins exactly that layout, because it is the one this originally got wrong.
pub fn success_only(body: &str) -> String {
    let without_comments: String = body
        .lines()
        .map(|l| match l.find("//") {
            Some(at) => &l[..at],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = without_comments;
    for call in ["detail_err(", "pg_err("] {
        while let Some(at) = out.find(call) {
            // The anchor ends in `(`, so the scan starts on the call's own paren.
            let open = at + call.len() - 1;
            let Some(close) = balanced_close(&out, open) else {
                break;
            };
            out.replace_range(at..=close, "");
        }
    }
    // `list_orgs` and `get_job` build their error bodies as an inline
    // `Json(json!({ "detail": … }))` rather than through `detail_err`, so the
    // two calls above leave those in place and `detail` then reads as the
    // success key. Remove the whole `Json(...)` call, anchored on its paren.
    while let Some(at) = find_inline_detail_error(&out) {
        let open = at + "Json".len();
        let Some(close) = balanced_close(&out, open) else {
            break;
        };
        out.replace_range(at..=close, "");
    }
    out
}

/// Index of the next `Json(json!({ "detail"` — an error body written inline.
fn find_inline_detail_error(src: &str) -> Option<usize> {
    let mut from = 0usize;
    while let Some(rel) = src[from..].find("Json(json!(") {
        let at = from + rel;
        let after = &src[at + "Json(json!(".len()..];
        let trimmed = after.trim_start();
        if let Some(inner) = trimmed.strip_prefix('{')
            && inner.trim_start().starts_with("\"detail\"")
        {
            return Some(at);
        }
        from = at + "Json(json!(".len();
    }
    None
}

/// Index of the `)` closing the `(` at `open`, if the source is balanced.
fn balanced_close(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    for (j, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
    }
    None
}

/// Classify a handler's success envelope, using its enclosing source to resolve
/// local struct constructors. Unknown values retain the scalar classification;
/// a known local representation that cannot be read is an error.
pub fn classify_envelope(body: &str, source: &str) -> Result<Envelope, String> {
    let src = success_only(body);
    // Checked before the JSON forms: an Arrow handler still calls `Json(...)`
    // on its error paths, and the body is not JSON.
    if src.contains("StreamWriter") {
        return Ok(Envelope::Arrow);
    }
    if src.contains("page_envelope(") {
        return Ok(Envelope::Page);
    }
    // `Json(json!({…}))` — every top-level key of the object literal.
    let keys = object_keys(&src);
    if !keys.is_empty() {
        return Ok(Envelope::Object(keys));
    }
    // `Json(json!(ident))` — a bare value, which for a list read is an array.
    if find_call(&src, "Json(json!(").is_some() {
        return Ok(Envelope::Bare);
    }
    if let Some(envelope) = typed_object(&src, source)? {
        return Ok(envelope);
    }
    if src.contains("Json(") {
        return Ok(Envelope::Scalar);
    }
    Err("no Json(...), page_envelope(...) or object literal found".to_string())
}

/// Resolve only an immediate named constructor. A variable or helper call needs
/// data-flow information this classifier does not have, so it stays unresolved.
fn typed_object(body: &str, source: &str) -> Result<Option<Envelope>, String> {
    let Some(syn::Expr::Struct(value)) = body.parse().ok().and_then(json_argument) else {
        return Ok(None);
    };
    let Some(name) = value.path.get_ident() else {
        return Ok(None);
    };
    let file = syn::parse_file(source).map_err(|e| format!("parse response declarations: {e}"))?;
    let Some(declaration) = file.items.iter().find_map(|item| match item {
        syn::Item::Struct(item) if item.ident == *name => Some(item),
        _ => None,
    }) else {
        return Ok(None);
    };
    // struct_fields deliberately reads Rust names. Serialization transforms or
    // conditional fields would make those names an incorrect wire contract.
    if declaration
        .attrs
        .iter()
        .chain(declaration.fields.iter().flat_map(|field| &field.attrs))
        .any(|attr| {
            ["serde", "cfg", "cfg_attr"]
                .iter()
                .any(|name| attr.path().is_ident(name))
        })
    {
        return Err(format!(
            "response struct {name} has unsupported serialization or conditional attributes"
        ));
    }
    if declaration.fields.is_empty() {
        // An empty json! object also has no wrapper keys.
        return Ok(Some(Envelope::Bare));
    }
    let fields = struct_fields(source, &name.to_string()).ok_or_else(|| {
        format!("response struct {name} is outside the supported local field syntax")
    })?;
    Ok(Some(Envelope::Object(
        fields
            .into_iter()
            .map(|field| field.name.trim_start_matches("r#").to_string())
            .collect(),
    )))
}

/// Token groups keep parentheses inside quoted/raw strings out of the delimiter
/// count. The enclosing body can be a source fragment rather than a valid block.
fn json_argument(tokens: proc_macro2::TokenStream) -> Option<syn::Expr> {
    use proc_macro2::{Delimiter, TokenTree};
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        if let TokenTree::Ident(name) = &token
            && name == "Json"
            && let Some(TokenTree::Group(group)) = tokens.peek()
            && group.delimiter() == Delimiter::Parenthesis
        {
            return syn::parse2(group.stream()).ok();
        }
        if let TokenTree::Group(group) = token
            && let Some(value) = json_argument(group.stream())
        {
            return Some(value);
        }
    }
    None
}

/// Every **top-level** key of a `Json(json!({ … }))` body, in source order.
///
/// Depth-tracked: a nested object's keys belong to a value, not to the
/// response's own shape, so `{"snapshot": {"run_id": …}}` is one key and not
/// two. Returns empty when the macro body is not an object literal.
pub fn object_keys(src: &str) -> Vec<String> {
    let Some(at) = find_call(src, "Json(json!(") else {
        return Vec::new();
    };
    let rest = &src[at..];
    // The literal must open immediately inside the macro; a `{` further along
    // belongs to something else (a closure, a match arm).
    let Some(brace) = rest.find('{') else {
        return Vec::new();
    };
    if rest[..brace].chars().any(|c| !c.is_whitespace()) {
        return Vec::new();
    }
    let body = &rest[brace + 1..];
    let bytes = body.as_bytes();
    let mut keys = Vec::new();
    let mut depth = 0i32;
    let mut i = 0usize;
    let mut expecting_key = true;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => {
                if depth == 0 {
                    break; // the literal closed
                }
                depth -= 1;
            }
            b',' if depth == 0 => expecting_key = true,
            b'"' if depth == 0 && expecting_key => {
                let start = i + 1;
                let Some(rel) = body[start..].find('"') else {
                    break;
                };
                let key = &body[start..start + rel];
                // A key is followed by `:`; a bare string value is not.
                let after = body[start + rel + 1..].trim_start();
                if after.starts_with(':') {
                    keys.push(key.to_string());
                }
                expecting_key = false;
                i = start + rel;
            }
            _ => {}
        }
        i += 1;
    }
    keys
}

/// Byte offset just past `needle`, for the first occurrence.
fn find_call(src: &str, needle: &str) -> Option<usize> {
    src.find(needle).map(|i| i + needle.len())
}

/// The ledger as the committed JSON: one entry per routed GET, path-sorted.
pub fn envelope_ledger(src: &str) -> Result<String, Vec<String>> {
    let bodies = handler_bodies(src);
    let routes = get_routes(src);
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    for (path, handler) in &routes {
        let Some(body) = bodies.get(handler) else {
            failures.push(format!(
                "GET {path}: handler `{handler}` has no body in lib.rs — the parser is wrong, \
                 or the handler moved to another module and this test must follow it"
            ));
            continue;
        };
        match classify_envelope(body, src) {
            Ok(env) => lines.push(format!("  \"GET {path}\": {}", env.as_json())),
            Err(why) => failures.push(format!(
                "GET {path} [{handler}]: {why}. Classify it rather than skipping it — a gate \
                 that drops what it cannot read reports all-clear over the novel case."
            )),
        }
    }
    if !failures.is_empty() {
        return Err(failures);
    }
    Ok(format!("{{\n{}\n}}\n", lines.join(",\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_classifier_reads_each_form() {
        let source = r"
struct JobsResponse {
    jobs: Vec<Job>,
}
struct OrgResponse {
    id: String,
    name: String,
}
struct EmptyResponse {
}
";
        let classify_envelope = |body| super::classify_envelope(body, source);
        // Each arm, against the shapes the handlers actually use. Without this the
        // classifier's branches are only exercised through whichever form happens
        // to appear first in lib.rs.
        assert_eq!(
            classify_envelope("{ Ok(rows) => Json(json!(rows)).into_response() }"),
            Ok(Envelope::Bare)
        );
        assert_eq!(
            classify_envelope("{ Json(json!({ \"jobs\": jobs })).into_response() }"),
            Ok(Envelope::Object(vec!["jobs".to_string()]))
        );
        assert_eq!(
            classify_envelope("{ Json(JobsResponse { jobs }).into_response() }"),
            classify_envelope("{ Json(json!({ \"jobs\": jobs })).into_response() }")
        );
        // Serialize follows the declaration, even when the constructor is reordered.
        assert_eq!(
            classify_envelope("{ Json(OrgResponse { name: o.name, id: o.id }) }"),
            classify_envelope("{ Json(json!({ \"id\": o.id, \"name\": o.name })) }")
        );
        assert_eq!(
            classify_envelope(r#"{ Json(OrgResponse { id: ")".into(), name: name }) }"#),
            Ok(Envelope::Object(vec!["id".into(), "name".into()]))
        );
        assert_eq!(
            classify_envelope("{ Json(EmptyResponse {}) }"),
            classify_envelope("{ Json(json!({})) }")
        );
        assert_eq!(
            classify_envelope(
                "{ match r { Ok(jobs) => Json(JobsResponse { jobs }), \
                 Err(e) => detail_err(StatusCode::NOT_FOUND, \"no\") } }"
            ),
            Ok(Envelope::Object(vec!["jobs".into()]))
        );
        assert_eq!(
            classify_envelope("{ Json(ExternalResponse { jobs }) }"),
            Ok(Envelope::Scalar)
        );
        // Several keys is a record, not a wrapper named by the first of them.
        assert_eq!(
            classify_envelope(
                "{ Json(json!({ \"id\": o.id, \"name\": o.name })).into_response() }"
            ),
            Ok(Envelope::Object(vec!["id".to_string(), "name".to_string()]))
        );
        // A nested object's keys belong to a value, not to the response shape.
        assert_eq!(
            classify_envelope("{ Json(json!({ \"snapshot\": { \"run_id\": r, \"at\": t } })) }"),
            Ok(Envelope::Object(vec!["snapshot".to_string()]))
        );
        assert_eq!(
            classify_envelope("{ page_envelope(read::imports_list(&s, snap, src, after, limit)) }"),
            Ok(Envelope::Page)
        );
        assert_eq!(
            classify_envelope("{ Ok(m) => Json(m).into_response() }"),
            Ok(Envelope::Scalar)
        );
        assert_eq!(
            classify_envelope("{ let mut w = StreamWriter::try_new(&mut buf, &page.schema); }"),
            Ok(Envelope::Arrow)
        );
        // An error arm must not be read as the success envelope — and removing it
        // must not take the success arm with it when they share a line, which is
        // what a line-based filter did here before.
        assert_eq!(
            classify_envelope(
                "{ match r { Ok(rows) => Json(json!(rows)).into_response(), \
             Err(e) => detail_err(StatusCode::NOT_FOUND, \"no\") } }"
            ),
            Ok(Envelope::Bare)
        );
        // The same, with the arms on separate lines.
        assert_eq!(
            classify_envelope(
                "{\n  Ok(rows) => Json(json!(rows)).into_response(),\n  \
             Err(e) => detail_err(StatusCode::NOT_FOUND, \"no\"),\n}"
            ),
            Ok(Envelope::Bare)
        );
        // An error-only handler has no success envelope to read.
        assert!(classify_envelope("{ detail_err(StatusCode::NOT_FOUND, \"no\") }").is_err());
        assert!(classify_envelope("{ StatusCode::NO_CONTENT.into_response() }").is_err());
    }

    #[test]
    fn unsupported_local_representations_are_not_silently_scalar() {
        for source in [
            "\n#[serde(rename_all = \"camelCase\")]\nstruct Reply {\n    job_ids: Vec<Job>,\n}\n",
            "\nstruct Reply {\n    #[serde(rename = \"items\")]\n    jobs: Vec<Job>,\n}\n",
            "\nstruct Reply {\n    #[serde(flatten)]\n    jobs: Vec<Job>,\n}\n",
            "\nstruct Reply {\n    #[serde(skip_serializing)]\n    jobs: Vec<Job>,\n}\n",
            "\nstruct Reply {\n    #[cfg(feature = \"extra\")]\n    jobs: Vec<Job>,\n}\n",
            "struct Reply { jobs: Vec<Job> }",
        ] {
            assert!(
                classify_envelope("{ Json(Reply { jobs }) }", source).is_err(),
                "unreadable local declaration must fail: {source}"
            );
        }
    }
}
