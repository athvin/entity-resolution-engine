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

use er_api_spec::{Tool, derive_tools, manifest_json, path_params, routes};
use std::collections::BTreeMap;

const SERVER_SRC: &str = include_str!("../src/lib.rs");
const COMMITTED: &str = include_str!("../../er-mcp/tools.json");

/// Every `.route("<path>", method(handler))` the router registers.
///
/// The shared walk in `er-api-spec` came from `mock_contract.rs`, not a regex: a regex with a
/// "next `.route(` follows" lookahead silently drops every registration that
/// chains two methods onto one path, and a route this test cannot see is a
/// route it reports as covered.
fn router_paths() -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for route in routes(SERVER_SRC) {
        for verb in ["get", "post", "put", "patch", "delete"] {
            if route.handlers.contains(&format!("{verb}(")) {
                out.entry(route.path.to_string())
                    .or_default()
                    .push(verb.to_ascii_uppercase());
            }
        }
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
        derive_tools(SERVER_SRC)
            .into_iter()
            .fold(BTreeMap::new(), |mut acc, t| {
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
    let tools = derive_tools(SERVER_SRC);
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
    for t in derive_tools(SERVER_SRC) {
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
    let tools = derive_tools(SERVER_SRC);
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
    let built = manifest_json(&derive_tools(SERVER_SRC));
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
    let built = manifest_json(&derive_tools(SERVER_SRC));
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../er-mcp/tools.json");
    std::fs::write(path, built).expect("write the manifest");
}
