//! The shared router walk. Interpretation stays separate so the existing gates
//! retain their method/handler rules while consuming the same registrations.

use std::collections::{BTreeMap, BTreeSet};

/// A router registration borrowing its literal path and handler expression.
#[derive(Debug, PartialEq, Eq)]
pub struct RouteRegistration<'a> {
    /// The path as written, including named parameters and colon actions.
    pub path: &'a str,
    /// The expression after the path, including any chained method handlers.
    pub handlers: &'a str,
}

/// The HTTP verbs a route registration can name (`get(h)`, qualified `patch(h)`, …).
const VERBS: [&str; 5] = ["get", "post", "put", "patch", "delete"];

/// Parse the router's `.route("<path>", <handlers>)` registrations.
///
/// Hand-scanned rather than pattern-matched: the registrations are interleaved
/// with comments and span lines freely, and paren-counting is indifferent to
/// both. (A regex with a "next `.route(` follows" lookahead silently drops every
/// registration preceded by a comment — it dropped five here.)
pub fn routes(src: &str) -> Vec<RouteRegistration<'_>> {
    const OPEN: &str = ".route(";
    let bytes = src.as_bytes();
    let mut out = Vec::new();
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
        out.push(RouteRegistration {
            path: &src[path_start..path_end],
            handlers: &src[path_end + 1..j.saturating_sub(1)],
        });
        i = j.max(after_open);
    }
    out
}

/// The verbs named in a `.route(path, …)` handler expression. Matched as whole
/// identifiers directly followed by `(`, so `get(get_org)` yields one `GET` and
/// `axum::routing::patch(h)` yields `PATCH`.
pub fn verbs_in(handler_expr: &str) -> BTreeSet<String> {
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

/// The handler named by `get(<ident>)` in a route's handler expression.
pub fn get_handler_in(expr: &str) -> Option<String> {
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

/// `GET <path>` -> handler name, from the router's registrations. Paren-counted
/// rather than pattern-matched, for the reason `mock_contract.rs` records: the
/// registrations are interleaved with comments that a regex silently skips.
pub fn get_routes(src: &str) -> BTreeMap<String, String> {
    routes(src)
        .into_iter()
        .filter_map(|route| {
            get_handler_in(route.handlers).map(|handler| (route.path.to_string(), handler))
        })
        .collect()
}
