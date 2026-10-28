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
        // A body is needed when the route takes one and the manifest cannot
        // say what shape it is. GET and DELETE never do; a body-less POST
        // (suspend, resume, retry) is fully described by its path.
        let needs_body = matches!(method.as_str(), "POST" | "PUT" | "PATCH")
            && !BODYLESS_MUTATIONS.contains(&path.as_str());

        out.push(Tool {
            name: tool_name(&method, &path),
            method,
            path,
            role: role.to_string(),
            summary,
            params,
            needs_body,
        });
    }
    out.sort_by(|a, b| (&a.method, &a.path).cmp(&(&b.method, &b.path)));
    out.dedup_by(|a, b| a.method == b.method && a.path == b.path);
    out
}

/// Mutations whose whole input is their path — no body to describe, so they
/// are fully derivable and ship as callable tools.
const BODYLESS_MUTATIONS: [&str; 6] = [
    "/v1/orgs/{org}/suspend",
    "/v1/orgs/{org}/resume",
    "/v1/orgs/{org}/jobs/{job_id}/cancel",
    "/v1/orgs/{org}/jobs/{job_id}/retry",
    "/v1/orgs/{org}/config/versions/{version}/activate",
    "/v1/orgs/{org}/staged/apply",
];

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
             \"summary\": {} }}{}",
            t.name,
            t.method,
            t.path,
            t.role,
            t.needs_body,
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
        .get("get_orgs_by_org_jobs")
        .expect("the org jobs list is a routed GET with a Viewer+ doc comment");
    assert_eq!(jobs.method, "GET");
    assert_eq!(jobs.role, "viewer");
    assert_eq!(jobs.params, vec!["org"]);
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
