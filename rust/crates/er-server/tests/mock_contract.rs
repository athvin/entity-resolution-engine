//! The mock-vs-router contract gate.
//!
//! `frontend/tests/mock-erserver/server.mjs` stands in for this server in the
//! Playwright tier: the browser never reaches er-server, so a BFF route's
//! upstream call is answered by the mock. That makes the mock a *claim about
//! this server's API* — and an unchecked one. When the mock serves a route the
//! real router does not, every frontend tier passes while the product fails in
//! a real deployment.
//!
//! That is not hypothetical. The CSV upload path (`POST /v1/orgs/{org}/imports`)
//! had a complete BFF handler and a mock that answered it, while the router
//! registered only `get(imports_list)` and the server had no multipart support
//! at all. It was green in every tier and 405d in the cluster. This test is the
//! gate that would have caught it, so the class cannot come back.
//!
//! Direction matters: the mock must not serve what the router lacks. The
//! converse is fine and expected — most routes are not exercised by frontend
//! tests.
//!
//! Runs under plain `make check` (no Postgres, no network): both inputs are
//! source text compiled in.

use std::collections::{BTreeMap, BTreeSet};

/// This server's router, as source. The route table is the authority the mock is
/// checked against.
const ROUTER_SRC: &str = include_str!("../src/lib.rs");

/// The frontend's stand-in for this server.
const MOCK_SRC: &str = include_str!("../../../../frontend/tests/mock-erserver/server.mjs");

/// The HTTP verbs a route registration can name, as they appear in the builder
/// (`get(h)`, `axum::routing::patch(h)`, …).
const VERBS: [&str; 5] = ["get", "post", "put", "patch", "delete"];

/// Path shape -> methods served at it. `{param}` names are erased so the two
/// sources' different spellings (`{org}` vs a regex capture) compare equal.
type Routes = BTreeMap<String, BTreeSet<String>>;

/// Erase path-parameter names: `/v1/orgs/{org}/jobs/{job_id}` -> `/v1/orgs/{}/jobs/{}`.
fn erase_params(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut depth = 0usize;
    for ch in path.chars() {
        match ch {
            '{' => {
                depth += 1;
                if depth == 1 {
                    out.push_str("{}");
                }
            }
            '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

/// The verbs named in a `.route(path, …)` handler expression. Matched as whole
/// identifiers directly followed by `(`, so `get(get_org)` yields one `GET` and
/// `axum::routing::patch(h)` yields `PATCH`.
fn verbs_in(handler_expr: &str) -> BTreeSet<String> {
    let bytes = handler_expr.as_bytes();
    let mut found = BTreeSet::new();
    for verb in VERBS {
        let mut from = 0usize;
        while let Some(rel) = handler_expr[from..].find(verb) {
            let at = from + rel;
            let after = at + verb.len();
            let boundary_before = at == 0 || {
                let p = bytes[at - 1];
                !p.is_ascii_alphanumeric() && p != b'_'
            };
            let call = handler_expr[after..].starts_with('(');
            if boundary_before && call {
                found.insert(verb.to_uppercase());
                break;
            }
            from = after;
        }
    }
    found
}

/// Parse the router's `.route("<path>", <handlers>)` registrations.
///
/// Hand-scanned rather than pattern-matched: the registrations are interleaved
/// with comments and span lines freely, and paren-counting is indifferent to
/// both. (A regex with a "next `.route(` follows" lookahead silently drops every
/// registration preceded by a comment — it dropped five here.)
fn router_routes(src: &str) -> Routes {
    const OPEN: &str = ".route(";
    let bytes = src.as_bytes();
    let mut out: Routes = BTreeMap::new();
    let mut i = 0usize;
    while let Some(rel) = src[i..].find(OPEN) {
        let after_open = i + rel + OPEN.len();
        // The path literal is the first quoted string inside the call.
        let Some(q1) = src[after_open..].find('"') else {
            break;
        };
        let path_start = after_open + q1 + 1;
        let Some(q2) = src[path_start..].find('"') else {
            break;
        };
        let path_end = path_start + q2;
        // The handler expression runs to the `)` that closes this `.route(`.
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
        let verbs = verbs_in(&src[path_end + 1..j.saturating_sub(1)]);
        if !verbs.is_empty() {
            out.entry(erase_params(&src[path_start..path_end]))
                .or_default()
                .extend(verbs);
        }
        i = j.max(after_open);
    }
    out
}

/// Expand a literal alternation group into one shape per alternative:
/// `/records/(protect|unprotect)` is two real paths, not a parameter.
fn expand_alternations(shape: &str) -> Vec<String> {
    let Some(open) = shape.find('(') else {
        return vec![shape.to_string()];
    };
    let Some(close_rel) = shape[open..].find(')') else {
        return vec![shape.to_string()];
    };
    let close = open + close_rel;
    let group = &shape[open + 1..close];
    let literal_alternation = group.contains('|')
        && group
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '|' || c == '-' || c == '_');
    if !literal_alternation {
        // Collapse this group to a parameter and keep scanning after it.
        let head = format!("{}{{}}", &shape[..open]);
        return expand_alternations(&shape[close + 1..])
            .into_iter()
            .map(|tail| format!("{head}{tail}"))
            .collect();
    }
    let mut out = Vec::new();
    for alt in group.split('|') {
        let head = format!("{}{alt}", &shape[..open]);
        for tail in expand_alternations(&shape[close + 1..]) {
            out.push(format!("{head}{tail}"));
        }
    }
    out
}

/// Parse the routes the mock answers, from its
/// `/^…$/.exec(path)) && req.method === "M"` guards (the method check wraps onto
/// the following line in ~13 of them, so a window is scanned, not a line).
fn mock_routes(src: &str) -> Routes {
    const EXEC: &str = "$/.exec(path)";
    let mut out: Routes = BTreeMap::new();
    let mut i = 0usize;
    while let Some(rel) = src[i..].find(EXEC) {
        let at = i + rel;
        i = at + EXEC.len();
        // Walk back to this literal's `/^` opener.
        let Some(open) = src[..at].rfind("/^") else {
            continue;
        };
        let pattern = &src[open + 2..at];
        // The method is the next `req.method === "…"` within the guard.
        const NEEDLE: &str = "req.method === \"";
        let window_end = (i + 200).min(src.len());
        let Some(m_rel) = src[i..window_end].find(NEEDLE) else {
            continue;
        };
        let m_start = i + m_rel + NEEDLE.len();
        let Some(m_len) = src[m_start..].find('"') else {
            continue;
        };
        let method = src[m_start..m_start + m_len].to_string();

        for shape in expand_alternations(&pattern.replace("\\/", "/")) {
            // The server accepts the BFF's `{id}:action` spelling by rewriting it
            // before routing, so normalize through the real rule.
            let shape = er_server::rewrite_action_path(&shape).unwrap_or(shape);
            out.entry(shape).or_default().insert(method.clone());
        }
    }
    out
}

#[test]
fn the_mock_serves_no_route_the_router_lacks() {
    let router = router_routes(ROUTER_SRC);
    let mock = mock_routes(MOCK_SRC);

    // Guard the parsers themselves: a silent regression to zero would make this
    // gate vacuous, which is the failure mode it exists to prevent.
    assert!(
        router.len() > 40,
        "router parse looks broken: {} shapes",
        router.len()
    );
    assert!(
        mock.len() > 30,
        "mock parse looks broken: {} shapes",
        mock.len()
    );

    let mut violations: Vec<String> = Vec::new();
    for (shape, methods) in &mock {
        for method in methods {
            match router.get(shape) {
                None => violations.push(format!(
                    "{method} {shape} — the mock answers it; the router has no such path"
                )),
                Some(served) if !served.contains(method) => violations.push(format!(
                    "{method} {shape} — the mock answers it; the router serves only {}",
                    served.iter().cloned().collect::<Vec<_>>().join(", ")
                )),
                Some(_) => {}
            }
        }
    }

    assert!(
        violations.is_empty(),
        "the frontend mock claims {} route(s) this server does not serve, so the \
         Playwright tier is green on an API that does not exist:\n  {}\n\nEither \
         implement the route here or stop answering it in \
         frontend/tests/mock-erserver/server.mjs.",
        violations.len(),
        violations.join("\n  ")
    );
}
