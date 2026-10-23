//! The response-envelope ledger.
//!
//! `mock_contract.rs` compares route and method; `wire-contract.test.ts` and
//! `mock-fabrication.test.ts` compare *field names*. None of them can see an
//! **envelope** mismatch — a list read as a bare array when the server wraps it
//! in `{key: […]}`. That is W0.5b: the BFF declared
//! `forward<unknown[]>("/v1/orgs/{org}/jobs")`, er-server sends `{jobs: […]}`,
//! and the dashboard crashed against the real backend while every mock-backed
//! tier was green, because the mock answered with the shape the frontend
//! believed in.
//!
//! The reason that was an easy mistake to make is worth stating, because it is a
//! property of this API rather than of that developer: **the two conventions are
//! genuinely mixed.** `/audit`, `/runs` and `/webhooks` return bare arrays;
//! `/jobs`, `/reviews` and `/webhooks/deliveries` wrap. Within one resource
//! family, both spellings occur, so there is no convention to infer and guessing
//! is a coin flip. Unifying them is a breaking change for every tenant reading
//! the API directly, so this test does not ask for that — it writes the fact
//! down, per route, where a consumer can check it.
//!
//! The ledger is **derived from the handlers**, never declared beside them. That
//! distinction is the point: a `#[utoipa::path]` response body, or any
//! hand-maintained table, is one more claim that can drift from the code — the
//! same shape as the defect it would be built to prevent. Here the source is the
//! authority, so the ledger cannot disagree with what the server returns without
//! the classifier failing.
//!
//! An envelope this test **cannot** classify is a hard failure naming the
//! handler, not a skip. A gate that silently drops what it does not understand
//! reports "all clear" over exactly the novel case most likely to be wrong.

use std::collections::BTreeMap;

/// This server's router and handlers, as source.
const SERVER_SRC: &str = include_str!("../src/lib.rs");

/// The committed ledger the frontend reads.
const COMMITTED: &str = include_str!("../../../../frontend/src/lib/api/envelopes.json");

/// How a successful list/detail response is shaped.
#[derive(Debug, PartialEq, Eq)]
enum Envelope {
    /// The value serialized directly, with **no wrapper key**:
    /// `Json(json!(rows))`. Deliberately not called "array": the source cannot
    /// tell a `Vec` from a single record here — `Json(json!(out))` in
    /// `list_orgs` is a list and `Json(json!(job))` in `get_job` is one record,
    /// and nothing in the response construction distinguishes them. What this
    /// variant does claim is the part W0.5b turned on: there is no envelope to
    /// unwrap. Whether the payload is a list follows from the resource, which
    /// the consumer already knows.
    Bare,
    /// A JSON object literal, carrying **all** its top-level keys. The key
    /// count is what separates the two cases the first draft of this test
    /// conflated: one key is a *wrapper* around a payload
    /// (`{"jobs": […]}`), several keys is a *record* built inline
    /// (`{"id": …, "name": …, "state": …}` in `get_org`). Reporting only the
    /// first key called the second one a wrapper named `id`, which is not a
    /// thing any consumer should be told.
    Object(Vec<String>),
    /// The shared keyset-page helper: `{items, next_cursor}`.
    Page,
    /// A single JSON object from a typed value: `Json(typed)`.
    Scalar,
    /// Not JSON at all: an Arrow IPC stream, whose snapshot and next cursor
    /// travel in response **headers** rather than in an envelope. A consumer
    /// that reaches for `body.items` or `body.next_cursor` here finds neither.
    Arrow,
}

impl Envelope {
    fn as_json(&self) -> String {
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
fn handler_bodies(src: &str) -> BTreeMap<String, String> {
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

/// `GET <path>` -> handler name, from the router's registrations. Paren-counted
/// rather than pattern-matched, for the reason `mock_contract.rs` records: the
/// registrations are interleaved with comments that a regex silently skips.
fn get_routes(src: &str) -> BTreeMap<String, String> {
    const OPEN: &str = ".route(";
    let bytes = src.as_bytes();
    let mut out = BTreeMap::new();
    let mut i = 0usize;
    while let Some(rel) = src[i..].find(OPEN) {
        let after_open = i + rel + OPEN.len();
        let Some(q1) = src[after_open..].find('"') else {
            break;
        };
        let path_start = after_open + q1 + 1;
        let Some(q2) = src[path_start..].find('"') else {
            break;
        };
        let path_end = path_start + q2;
        let mut depth = 1usize;
        let mut j = path_end + 1;
        while j < bytes.len() && depth > 0 {
            match bytes[j] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        let handlers = &src[path_end + 1..j.saturating_sub(1)];
        if let Some(h) = get_handler_in(handlers) {
            out.insert(src[path_start..path_end].to_string(), h);
        }
        i = j.max(after_open);
    }
    out
}

/// The handler named by `get(<ident>)` in a route's handler expression.
fn get_handler_in(expr: &str) -> Option<String> {
    let bytes = expr.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = expr[from..].find("get(") {
        let at = from + rel;
        let boundary = at == 0 || {
            let p = bytes[at - 1];
            !p.is_ascii_alphanumeric() && p != b'_'
        };
        if boundary {
            let ident: String = expr[at + 4..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !ident.is_empty() {
                return Some(ident);
            }
        }
        from = at + 4;
    }
    None
}

/// Strip the error arms, so an error envelope is never mistaken for the success
/// one. `detail_err(...)` and `pg_err(...)` build `{"detail": …}`, which would
/// otherwise classify every handler as `Object("detail")`.
///
/// Each call is removed by **balanced parens**, not by dropping its line: a
/// handler whose match arms share one line would otherwise lose its success arm
/// along with its error arm and be reported unclassifiable. The unit test below
/// pins exactly that layout, because it is the one this originally got wrong.
fn success_only(body: &str) -> String {
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

/// Classify a handler's success envelope, or return why it could not be read.
fn classify(body: &str) -> Result<Envelope, String> {
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
    if src.contains("Json(") {
        return Ok(Envelope::Scalar);
    }
    Err("no Json(...), page_envelope(...) or object literal found".to_string())
}

/// Every **top-level** key of a `Json(json!({ … }))` body, in source order.
///
/// Depth-tracked: a nested object's keys belong to a value, not to the
/// response's own shape, so `{"snapshot": {"run_id": …}}` is one key and not
/// two. Returns empty when the macro body is not an object literal.
fn object_keys(src: &str) -> Vec<String> {
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
fn ledger() -> Result<String, Vec<String>> {
    let bodies = handler_bodies(SERVER_SRC);
    let routes = get_routes(SERVER_SRC);
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
        match classify(body) {
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

#[test]
fn every_routed_get_has_a_classified_envelope() {
    let routes = get_routes(SERVER_SRC);
    // A parser that matched nothing would make the ledger trivially correct.
    assert!(
        routes.len() > 30,
        "parsed only {} GET routes from the router; the parser is broken",
        routes.len()
    );
    match ledger() {
        Ok(_) => {}
        Err(failures) => panic!(
            "{} routed GET handler(s) could not be classified:\n{}",
            failures.len(),
            failures.join("\n")
        ),
    }
}

#[test]
fn the_committed_ledger_is_current() {
    let built = ledger().unwrap_or_else(|f| panic!("{}", f.join("\n")));
    assert_eq!(
        built, COMMITTED,
        "frontend/src/lib/api/envelopes.json is stale. It is generated from the handlers, so a \
         diff here means a response envelope changed — check every BFF route and mock fixture \
         that reads the affected path before regenerating with \
         `cargo test -p er-server --test response_envelopes -- --ignored regenerate`"
    );
}

#[test]
fn both_conventions_really_are_in_use() {
    // The mixed convention is the premise of this test's existence. If the API
    // is ever unified, this fails and the doc comment above needs rewriting
    // rather than quietly becoming false.
    let built = ledger().unwrap_or_else(|f| panic!("{}", f.join("\n")));
    assert!(
        built.contains("\"envelope\": \"bare\""),
        "no un-enveloped GET remains"
    );
    assert!(
        built.contains("\"envelope\": \"wrapped\""),
        "no wrapped GET remains"
    );
}

#[test]
fn the_classifier_reads_each_form() {
    // Each arm, against the shapes the handlers actually use. Without this the
    // classifier's branches are only exercised through whichever form happens
    // to appear first in lib.rs.
    assert_eq!(
        classify("{ Ok(rows) => Json(json!(rows)).into_response() }"),
        Ok(Envelope::Bare)
    );
    assert_eq!(
        classify("{ Json(json!({ \"jobs\": jobs })).into_response() }"),
        Ok(Envelope::Object(vec!["jobs".to_string()]))
    );
    // Several keys is a record, not a wrapper named by the first of them.
    assert_eq!(
        classify("{ Json(json!({ \"id\": o.id, \"name\": o.name })).into_response() }"),
        Ok(Envelope::Object(vec!["id".to_string(), "name".to_string()]))
    );
    // A nested object's keys belong to a value, not to the response shape.
    assert_eq!(
        classify("{ Json(json!({ \"snapshot\": { \"run_id\": r, \"at\": t } })) }"),
        Ok(Envelope::Object(vec!["snapshot".to_string()]))
    );
    assert_eq!(
        classify("{ page_envelope(read::imports_list(&s, snap, src, after, limit)) }"),
        Ok(Envelope::Page)
    );
    assert_eq!(
        classify("{ Ok(m) => Json(m).into_response() }"),
        Ok(Envelope::Scalar)
    );
    assert_eq!(
        classify("{ let mut w = StreamWriter::try_new(&mut buf, &page.schema); }"),
        Ok(Envelope::Arrow)
    );
    // An error arm must not be read as the success envelope — and removing it
    // must not take the success arm with it when they share a line, which is
    // what a line-based filter did here before.
    assert_eq!(
        classify(
            "{ match r { Ok(rows) => Json(json!(rows)).into_response(), \
             Err(e) => detail_err(StatusCode::NOT_FOUND, \"no\") } }"
        ),
        Ok(Envelope::Bare)
    );
    // The same, with the arms on separate lines.
    assert_eq!(
        classify(
            "{\n  Ok(rows) => Json(json!(rows)).into_response(),\n  \
             Err(e) => detail_err(StatusCode::NOT_FOUND, \"no\"),\n}"
        ),
        Ok(Envelope::Bare)
    );
    // An error-only handler has no success envelope to read.
    assert!(classify("{ detail_err(StatusCode::NOT_FOUND, \"no\") }").is_err());
    assert!(classify("{ StatusCode::NO_CONTENT.into_response() }").is_err());
}

/// Rewrite the committed ledger. Run with
/// `cargo test -p er-server --test response_envelopes -- --ignored regenerate`.
#[test]
#[ignore = "writes frontend/src/lib/api/envelopes.json"]
fn regenerate() {
    let built = ledger().unwrap_or_else(|f| panic!("{}", f.join("\n")));
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../frontend/src/lib/api/envelopes.json"
    );
    std::fs::write(path, built).expect("write the ledger");
}
