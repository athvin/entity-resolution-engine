//! The MCP tool manifest, derived from the handlers.
//!
//! O10 was filed as wanting `#[utoipa::path]` on every handler first, because
//! `openapi.json` documents zero paths and "there is nothing for an MCP server
//! to generate tools from". That reading was wrong twice over.
//!
//! First, there IS something: every routed handler already carries a doc
//! comment in the shape `Role: what it does (`METHOD /path`)` — 81 of them,
//! written for humans and incidentally exactly what an MCP tool needs (a name,
//! a one-line description, the method and path, and the role required).
//!
//! Second, and more important: this repo has already decided against the
//! alternative, in `response_envelopes.rs`'s own words — "a `#[utoipa::path]`
//! response body, or any hand-maintained table, is one more claim that can
//! drift from the code — the same shape as the defect it would be built to
//! prevent." Hand-annotating 81 handlers would create 81 claims nothing checks.
//! Deriving from the source means the manifest cannot disagree with the server
//! without this test failing.
//!
//! **A routed handler this test cannot classify is a hard failure naming it,
//! not a skip** — the rule `response_envelopes.rs` states and for the same
//! reason: a gate that silently drops what it does not understand reports "all
//! clear" over exactly the novel case most likely to be wrong.

use std::collections::BTreeMap;

const SERVER_SRC: &str = include_str!("../src/lib.rs");
const COMMITTED: &str = include_str!("../../er-mcp/tools.json");

/// One routed handler, as the manifest records it.
#[derive(Debug, PartialEq, Eq)]
struct Tool {
    name: String,
    method: String,
    path: String,
    role: String,
    summary: String,
    /// Path parameters, in path order — every `{...}` segment.
    params: Vec<String>,
    /// Whether a call needs a request body this manifest cannot describe.
    needs_body: bool,
    /// The request body's JSON Schema, derived from the handler's `Json<T>`
    /// extractor (O10b). Empty when the route takes no body.
    body: String,
}

/// The role a doc comment's prefix names.
fn role_of(line: &str) -> Option<&'static str> {
    // The role is the first word before the colon, which may be QUALIFIED —
    // `Admin+ (operator for structural blocks):` is a real one. Matching the
    // whole literal `Admin+:` misses it, and a missed role means a routed
    // handler silently absent from the tool list.
    let head = line.split(':').next()?;
    let word = head.split_whitespace().next()?.trim_end_matches('+');
    match word {
        "Operator" => Some("operator"),
        "Admin" => Some("admin"),
        "Steward" => Some("steward"),
        "Viewer" => Some("viewer"),
        "Public" => Some("public"),
        _ => None,
    }
}

/// `{org}` / `{job_id}` segments, in path order.
fn path_params(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|seg| {
            seg.strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .map(str::to_string)
        })
        .collect()
}

/// A stable tool name: the method plus the path's literal segments.
///
/// Derived rather than hand-named so two routes cannot collide silently and a
/// renamed route shows up as a changed tool rather than a quietly repointed
/// one.
fn tool_name(method: &str, path: &str) -> String {
    let mut parts = vec![method.to_ascii_lowercase()];
    for seg in path.split('/') {
        if seg.is_empty() || seg == "v1" {
            continue;
        }
        // A `{param}` becomes `by_param`, NOT nothing. Dropping it collapses
        // `/v1/orgs` and `/v1/orgs/{org}` onto one name — two different tools
        // under one label, where a caller gets whichever the lookup finds
        // first.
        if let Some(param) = seg.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
            parts.push(format!("by_{}", param.replace('-', "_")));
        } else {
            parts.push(seg.replace(['-', '.', ':'], "_"));
        }
    }
    parts.join("_")
}

/// Every handler doc comment of the form `Role: summary (`METHOD /path`)`.
fn derive() -> Vec<Tool> {
    let lines: Vec<&str> = SERVER_SRC.lines().collect();
    let mut out: Vec<Tool> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let Some(doc) = line.trim_start().strip_prefix("/// ") else {
            continue;
        };
        let Some(role) = role_of(doc) else { continue };

        // The summary runs to the end of the sentence; the method and path sit
        // in backticks, possibly on the NEXT doc line when the first wraps.
        let mut block = doc.to_string();
        for next in lines.iter().skip(i + 1).take(3) {
            let Some(cont) = next.trim_start().strip_prefix("///") else {
                break;
            };
            block.push(' ');
            block.push_str(cont.trim());
        }

        let Some((method, path)) = method_and_path(&block) else {
            continue;
        };
        // One sentence. The doc comments are written for a human reading the
        // source and some run for a paragraph; an MCP client shows this in a
        // tool picker, where a paragraph is noise that crowds out the other
        // tools' names.
        let summary = block
            .split_once(':')
            .map_or(block.as_str(), |(_, rest)| rest.trim())
            .split(" (`")
            .next()
            .unwrap_or("")
            .split(". ")
            .next()
            .unwrap_or("")
            .trim()
            .trim_end_matches('.')
            .to_string();

        let params = path_params(&path);
        // O10b: a body is DESCRIBED when the handler's `Json<T>` resolves to a
        // schema. `needs_body` now means "takes a body this manifest could not
        // describe" — which, with the derivation below, is only a type the
        // mapping does not know, and that is a hard failure rather than a
        // silently untooled route.
        // Only a method that CAN carry a body gets one looked for. Without
        // this the forward scan wanders into the next handler's signature and
        // hands a GET a request body it cannot have — which `GET /healthz`
        // demonstrated.
        let can_have_body = matches!(method.as_str(), "POST" | "PUT" | "PATCH");
        let body = if can_have_body {
            body_type_after(&lines, i)
                .and_then(|t| body_schema(&t))
                .unwrap_or_default()
        } else {
            String::new()
        };
        // Whether a route takes a body is a property of its SIGNATURE, not a
        // list someone maintains: `body_type_after` finds the `Json<T>`
        // extractor or does not. The hand-kept list this replaced was already
        // wrong about four routes — publish, resume, activate and compile all
        // take their whole input from the path, and were being withheld from
        // the tool list for a body they never had.
        let non_json = can_have_body && NON_JSON_BODIES.contains(&path.as_str());
        let takes_body = !body.is_empty() || non_json;
        let needs_body = non_json;

        out.push(Tool {
            name: tool_name(&method, &path),
            method,
            path,
            role: role.to_string(),
            summary,
            params,
            needs_body,
            body: if takes_body { body } else { String::new() },
        });
    }
    out.sort_by(|a, b| (&a.method, &a.path).cmp(&(&b.method, &b.path)));
    out.dedup_by(|a, b| a.method == b.method && a.path == b.path);
    out
}

/// One field of a request-body struct.
struct Field {
    name: String,
    ty: String,
    /// `#[serde(default)]` — absent is legal, so the field is not required.
    optional: bool,
}

/// Parse `struct <name> { … }` out of the server source.
///
/// Returns `None` for a type that is not a local struct — `serde_json::Value`
/// is the real case, a passthrough body with no shape to describe.
fn struct_fields(name: &str) -> Option<Vec<Field>> {
    let header = format!("\nstruct {name} {{\n");
    let start = SERVER_SRC.find(&header)? + header.len();
    let end = start + SERVER_SRC[start..].find("\n}\n")?;
    let mut fields = Vec::new();
    let mut defaulted = false;
    for raw in SERVER_SRC[start..end].lines() {
        let line = raw.trim();
        if line.starts_with("///") || line.starts_with("//") || line.is_empty() {
            continue;
        }
        if line.starts_with("#[") {
            // `#[serde(default)]` and `#[serde(default, …)]` both mean the
            // field may be absent. Anything else on the attribute is about
            // serialisation, not presence.
            defaulted |= line.contains("default");
            continue;
        }
        let (field, ty) = line.trim_end_matches(',').split_once(": ")?;
        fields.push(Field {
            name: field.trim().trim_start_matches("pub ").to_string(),
            ty: ty.trim().to_string(),
            optional: defaulted || ty.trim().starts_with("Option<"),
        });
        defaulted = false;
    }
    Some(fields)
}

/// A Rust field type as JSON Schema.
///
/// Returns `None` for a type this mapping does not know. That is a HARD
/// FAILURE at the call site rather than a permissive `{}`: a schema that says
/// "anything" for a field the server will reject is worse than no tool, since
/// the model calls it confidently and reads a 422 it cannot act on.
fn type_schema(ty: &str) -> Option<String> {
    let inner = ty
        .strip_prefix("Option<")
        .and_then(|t| t.strip_suffix('>'))
        .unwrap_or(ty);
    Some(match inner {
        "String" => "{\"type\":\"string\"}".to_string(),
        "bool" => "{\"type\":\"boolean\"}".to_string(),
        "i64" | "usize" | "u32" | "u64" => "{\"type\":\"integer\"}".to_string(),
        "f64" => "{\"type\":\"number\"}".to_string(),
        // A passthrough body or free-form map: no shape to assert, and
        // asserting one would be a guess.
        "Value" | "serde_json::Value" => "{}".to_string(),
        "std::collections::BTreeMap<String, String>" => {
            "{\"type\":\"object\",\"additionalProperties\":{\"type\":\"string\"}}".to_string()
        }
        other => {
            let item = other
                .strip_prefix("Vec<")
                .and_then(|t| t.strip_suffix('>'))?;
            format!("{{\"type\":\"array\",\"items\":{}}}", type_schema(item)?)
        }
    })
}

/// The JSON Schema for a `Json<T>` request body.
///
/// Recurses one level into nested structs (`Vec<BulkResolveItem>`), because a
/// tool that says "an array of something" is one the model cannot fill in.
fn body_schema(type_name: &str) -> Option<String> {
    let Some(fields) = struct_fields(type_name) else {
        // Not a local struct — `Json<Value>`, a passthrough. Describable as
        // "an object", which is honest rather than a guess.
        return Some("{\"type\":\"object\"}".to_string());
    };
    let mut props: Vec<String> = Vec::new();
    let mut required: Vec<String> = Vec::new();
    for f in &fields {
        // A nested local struct recurses rather than giving up: a tool that
        // says "an array of something" is one the model cannot fill in.
        let schema = if let Some(s) = type_schema(&f.ty) {
            s
        } else {
            let item =
                f.ty.trim_start_matches("Option<")
                    .trim_end_matches('>')
                    .strip_prefix("Vec<")
                    .map(|t| t.trim_end_matches('>'))?;
            format!("{{\"type\":\"array\",\"items\":{}}}", body_schema(item)?)
        };
        props.push(format!("\"{}\":{schema}", f.name));
        if !f.optional {
            required.push(format!("\"{}\"", f.name));
        }
    }
    Some(format!(
        "{{\"type\":\"object\",\"properties\":{{{}}},\"required\":[{}],\"additionalProperties\":false}}",
        props.join(","),
        required.join(",")
    ))
}

/// The `Json<T>` body type a handler takes, if any.
///
/// Found by walking forward from the doc comment to the handler's signature —
/// the same adjacency the summary relies on.
fn body_type_after(lines: &[&str], doc_index: usize) -> Option<String> {
    let mut seen_fn = false;
    for line in lines.iter().skip(doc_index).take(40) {
        if line.starts_with("async fn ") || line.starts_with("fn ") {
            seen_fn = true;
        }
        if seen_fn && line.contains("Json<") {
            let after = line.split("Json<").nth(1)?;
            return Some(after.split('>').next()?.trim().to_string());
        }
        // The signature ends at its return type.
        if seen_fn && line.contains("-> ") {
            return None;
        }
    }
    None
}

/// Routes whose body is not JSON and so cannot become tool arguments.
///
/// One entry, and it is honest rather than an oversight: the CSV upload takes
/// `Multipart`, and a file is not something an MCP client can put in a tool
/// call. The JSON push (`records:ingest`) is the route an agent should use.
const NON_JSON_BODIES: [&str; 1] = ["/v1/orgs/{org}/imports"];

/// `(`GET /v1/orgs/{org}/jobs`)` → `("GET", "/v1/orgs/{org}/jobs")`.
fn method_and_path(block: &str) -> Option<(String, String)> {
    for chunk in block.split('`') {
        let mut it = chunk.split_whitespace();
        let method = it.next()?;
        if !matches!(method, "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
            continue;
        }
        let path = it.next()?;
        if path.starts_with('/') && it.next().is_none() {
            // Doc comments illustrate query parameters (`?key=…`); the ROUTE
            // is the path alone, and a tool named after the illustration would
            // address nothing.
            let path = path.split('?').next().unwrap_or(path);
            return Some((method.to_string(), path.to_string()));
        }
    }
    None
}

fn manifest_json(tools: &[Tool]) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("{\n  \"tools\": [\n");
    for (i, t) in tools.iter().enumerate() {
        let params = t
            .params
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "    {{ \"name\": \"{}\", \"method\": \"{}\", \"path\": \"{}\", \
             \"role\": \"{}\", \"params\": [{params}], \"needs_body\": {}, \
             \"body\": {}, \"summary\": {} }}{}",
            t.name,
            t.method,
            t.path,
            t.role,
            t.needs_body,
            if t.body.is_empty() {
                "null".to_string()
            } else {
                t.body.clone()
            },
            serde_json::to_string(&t.summary).expect("summary serialises"),
            if i + 1 == tools.len() { "" } else { "," }
        );
    }
    out.push_str("  ]\n}\n");
    out
}

/// Every `.route("<path>", method(handler))` the router registers.
///
/// The brace-matching walk is `mock_contract.rs`'s, not a regex: a regex with a
/// "next `.route(` follows" lookahead silently drops every registration that
/// chains two methods onto one path, and a route this test cannot see is a
/// route it reports as covered.
fn router_paths() -> BTreeMap<String, Vec<String>> {
    const OPEN: &str = ".route(";
    let bytes = SERVER_SRC.as_bytes();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut i = 0usize;
    while let Some(rel) = SERVER_SRC[i..].find(OPEN) {
        let after_open = i + rel + OPEN.len();
        let Some(q1) = SERVER_SRC[after_open..].find('"') else {
            break;
        };
        let path_start = after_open + q1 + 1;
        let Some(q2) = SERVER_SRC[path_start..].find('"') else {
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
        let handlers = &SERVER_SRC[path_end + 1..j.saturating_sub(1)];
        let path = SERVER_SRC[path_start..path_end].to_string();
        for verb in ["get", "post", "put", "patch", "delete"] {
            if handlers.contains(&format!("{verb}(")) {
                out.entry(path.clone())
                    .or_default()
                    .push(verb.to_ascii_uppercase());
            }
        }
        i = j.max(after_open);
    }
    out
}

/// The router spells a sub-action `:activate`; the doc comments and the
/// rewrite both normalise it to `/activate`.
fn normalise(path: &str) -> String {
    path.replace(':', "/")
}

#[test]
fn every_routed_handler_is_described() {
    // The rule this file's header states, enforced rather than asserted: a
    // handler the derivation cannot classify is a FAILURE naming it. Without
    // this the manifest would silently shrink as routes were added — reporting
    // "all clear" over exactly the new surface most likely to be wrong, which
    // is the shape the BFF-authz gate was bitten by.
    let described: BTreeMap<String, Vec<String>> =
        derive().into_iter().fold(BTreeMap::new(), |mut acc, t| {
            acc.entry(normalise(&t.path)).or_default().push(t.method);
            acc
        });

    let mut missing: Vec<String> = Vec::new();
    for (path, verbs) in router_paths() {
        // The §6.4 attempt channel is machine-to-machine: authenticated by the
        // per-attempt credential rather than a bearer principal, never
        // forwarded by the BFF, and not a tool an MCP caller could invoke —
        // describing it here would advertise a surface no principal can use.
        if path.starts_with("/v1/internal/") {
            continue;
        }
        let key = normalise(&path);
        for verb in verbs {
            let covered = described.get(&key).is_some_and(|m| m.contains(&verb));
            if !covered {
                missing.push(format!("{verb} {path}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{} routed handler(s) carry no `Role: summary (`METHOD /path`)` doc \
         comment, so an MCP caller is never told they exist:\n  {}\n\nAdd one \
         to each — the convention is the documentation, not an annotation \
         beside it.",
        missing.len(),
        missing.join("\n  ")
    );
}

#[test]
fn the_derivation_sees_real_handlers() {
    // A parser that matched nothing would make every assertion below vacuous —
    // the shape `response_envelopes.rs` and `mock_contract.rs` both guard.
    let tools = derive();
    assert!(
        tools.len() > 40,
        "the derivation found only {} tools; the doc-comment convention has \
         changed or the parser is broken",
        tools.len()
    );
    let by_name: BTreeMap<&str, &Tool> = tools.iter().map(|t| (t.name.as_str(), t)).collect();
    // `by_org`, not nothing: see `tool_name`. `/v1/orgs` and `/v1/orgs/{org}`
    // are different tools and must not share a name.
    let jobs = by_name
        .get("get_orgs_by_org_datasets_by_ds_jobs")
        .expect("the dataset jobs list is a routed GET with a Viewer+ doc comment");
    assert_eq!(jobs.method, "GET");
    assert_eq!(jobs.role, "viewer");
    assert_eq!(jobs.params, vec!["org", "ds"]);
    assert!(!jobs.needs_body);
    assert!(
        !jobs.summary.is_empty() && !jobs.summary.contains('`'),
        "the summary must be prose, not the signature: {:?}",
        jobs.summary
    );
}

#[test]
fn every_tool_has_a_usable_shape() {
    for t in derive() {
        assert!(!t.name.is_empty(), "{t:?} has no name");
        assert!(t.path.starts_with('/'), "{t:?} has a relative path");
        assert!(!t.summary.is_empty(), "{t:?} has no summary");
        // Every `{param}` in the path must be offered as an input, or a caller
        // cannot construct the URL at all.
        for p in path_params(&t.path) {
            assert!(t.params.contains(&p), "{t:?} does not offer `{p}`");
        }
    }
}

#[test]
fn tool_names_are_unique() {
    let tools = derive();
    let mut seen: BTreeMap<&str, &Tool> = BTreeMap::new();
    for t in &tools {
        if let Some(prev) = seen.insert(&t.name, t) {
            panic!(
                "tool name `{}` is claimed by both {prev:?} and {t:?}",
                t.name
            );
        }
    }
}

#[test]
fn the_committed_manifest_is_current() {
    let built = manifest_json(&derive());
    assert_eq!(
        built, COMMITTED,
        "rust/crates/er-mcp/tools.json is stale. It is DERIVED from the \
         handlers' doc comments, so a diff here means a route, role or summary \
         changed — check what an MCP caller would now be told before \
         regenerating with `cargo test -p er-server --test mcp_manifest -- \
         --ignored regenerate`"
    );
}

/// Rewrite the committed manifest. Run with
/// `cargo test -p er-server --test mcp_manifest -- --ignored regenerate`.
#[test]
#[ignore = "writes rust/crates/er-mcp/tools.json"]
fn regenerate() {
    let built = manifest_json(&derive());
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../er-mcp/tools.json");
    std::fs::write(path, built).expect("write the manifest");
}
