//! What a handler can answer besides success, derived from its source (#1333):
//! the error statuses it (or a local helper it calls) constructs, and the
//! request and response headers it reads or sets.
//!
//! The walk follows calls into functions defined in the same source file, so
//! `guard` (401/403/404), `upload_scope_any` (404), `idempotency_key` (the
//! request header) and `accepted_response` (the replay header) contribute to
//! every handler that uses them. It over-approximates — a helper's every
//! status counts, whichever branch the handler takes — which is the safe
//! direction for a contract: a client prepared for a status it never sees
//! loses nothing.

use std::collections::{BTreeMap, BTreeSet};

use syn::visit::Visit;

use super::Source;

/// The request and response headers the contract knows how to describe.
pub const REQUEST_HEADERS: &[(&str, &str)] = &[("idempotency-key", "Idempotency-Key")];
pub const RESPONSE_HEADERS: &[(&str, &str)] = &[("idempotent-replayed", "Idempotent-Replayed")];

/// A handler's non-success surface.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Surface {
    /// HTTP error statuses (4xx/5xx), ascending.
    pub statuses: BTreeSet<u16>,
    /// Canonical request header names it reads.
    pub request_headers: BTreeSet<&'static str>,
    /// Canonical response header names it sets.
    pub response_headers: BTreeSet<&'static str>,
}

/// `StatusCode::NAME` → its code, for the names this codebase uses.
fn code(name: &str) -> Option<u16> {
    Some(match name {
        "BAD_REQUEST" => 400,
        "UNAUTHORIZED" => 401,
        "PAYMENT_REQUIRED" => 402,
        "FORBIDDEN" => 403,
        "NOT_FOUND" => 404,
        "METHOD_NOT_ALLOWED" => 405,
        "NOT_ACCEPTABLE" => 406,
        "CONFLICT" => 409,
        "GONE" => 410,
        "LENGTH_REQUIRED" => 411,
        "PRECONDITION_FAILED" => 412,
        "PAYLOAD_TOO_LARGE" => 413,
        "UNSUPPORTED_MEDIA_TYPE" => 415,
        "RANGE_NOT_SATISFIABLE" => 416,
        "UNPROCESSABLE_ENTITY" => 422,
        "LOCKED" => 423,
        "PRECONDITION_REQUIRED" => 428,
        "TOO_MANY_REQUESTS" => 429,
        "INTERNAL_SERVER_ERROR" => 500,
        "NOT_IMPLEMENTED" => 501,
        "BAD_GATEWAY" => 502,
        "SERVICE_UNAVAILABLE" => 503,
        "GATEWAY_TIMEOUT" => 504,
        _ => return None,
    })
}

struct Walk<'a> {
    source: &'a Source<'a>,
    surface: Surface,
    seen: BTreeSet<String>,
    pending: Vec<String>,
}

impl<'ast> Visit<'ast> for Walk<'_> {
    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        let segments: Vec<String> = path
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        if let [.., ty, name] = segments.as_slice()
            && ty == "StatusCode"
            && let Some(c) = code(name)
        {
            self.surface.statuses.insert(c);
        }
        syn::visit::visit_expr_path(self, path);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(p) = call.func.as_ref()
            && let Some(ident) = p.path.get_ident()
        {
            let name = ident.to_string();
            if self.source.functions.contains_key(&name) && !self.seen.contains(&name) {
                self.pending.push(name);
            }
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_lit_str(&mut self, lit: &'ast syn::LitStr) {
        let value = lit.value().to_ascii_lowercase();
        for (raw, canonical) in REQUEST_HEADERS {
            if value == *raw {
                self.surface.request_headers.insert(canonical);
            }
        }
        for (raw, canonical) in RESPONSE_HEADERS {
            if value == *raw {
                self.surface.response_headers.insert(canonical);
            }
        }
    }
}

/// Derive `handler`'s surface, following local helpers transitively.
pub(super) fn derive(source: &Source<'_>, handler: &str) -> Surface {
    let mut walk = Walk {
        source,
        surface: Surface::default(),
        seen: BTreeSet::new(),
        pending: vec![handler.to_string()],
    };
    // An authenticated handler takes the `Principal` extractor, whose own
    // rejection — a missing or invalid credential — is the 401.
    if let Some(f) = source.functions.get(handler)
        && f.sig.inputs.iter().any(
            |arg| matches!(arg, syn::FnArg::Typed(t) if quote_ty(&t.ty).ends_with("Principal")),
        )
    {
        walk.surface.statuses.insert(401);
    }
    while let Some(name) = walk.pending.pop() {
        if !walk.seen.insert(name.clone()) {
            continue;
        }
        if let Some(f) = source.functions.get(&name) {
            walk.visit_block(&f.block);
        }
    }
    walk.surface
}

fn quote_ty(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// The canonical reason phrase for a status, for a response description.
#[must_use]
pub fn reason(code: u16) -> &'static str {
    let reasons: BTreeMap<u16, &str> = BTreeMap::from([
        (400, "The request is malformed."),
        (401, "No valid credential."),
        (402, "The org's record allowance is used up."),
        (
            403,
            "The credential's role is below what this operation needs.",
        ),
        (
            404,
            "Not found — including another org's resource, which is never distinguished from a missing one.",
        ),
        (405, "Method not allowed."),
        (406, "Not acceptable."),
        (
            409,
            "The request conflicts with the resource's current state.",
        ),
        (410, "Gone."),
        (411, "Length required."),
        (412, "Precondition failed."),
        (413, "The body is larger than this operation accepts."),
        (415, "Unsupported media type."),
        (416, "Range not satisfiable."),
        (
            422,
            "The request is well-formed but invalid; `detail` says which field and why.",
        ),
        (423, "Locked."),
        (428, "A precondition header is required."),
        (429, "Too many requests."),
        (500, "Internal error."),
        (
            501,
            "Not available on this deployment (a store it needs is not configured).",
        ),
        (502, "An upstream provider failed."),
        (
            503,
            "Temporarily unavailable, or not configured on this deployment.",
        ),
        (504, "An upstream provider timed out."),
    ]);
    reasons.get(&code).copied().unwrap_or("Error.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_and_headers_follow_local_helpers() {
        let src = r#"
            fn guard(p: &Principal) -> Result<(), Response> {
                if no() { return Err(detail_err(StatusCode::FORBIDDEN, "no")); }
                Ok(())
            }
            fn key(headers: &HeaderMap) -> Option<&str> { headers.get("idempotency-key") }
            fn receipt() -> Response { let mut h = HeaderMap::new(); h.insert("idempotent-replayed", v()); ok(h) }
            async fn handler(principal: Principal, headers: HeaderMap) -> Response {
                guard(&principal)?;
                let _ = key(&headers);
                if big() { return detail_err(axum::http::StatusCode::PAYLOAD_TOO_LARGE, "big"); }
                (StatusCode::ACCEPTED, receipt()).into_response()
            }
            async fn probe() -> &'static str { "ok" }
        "#;
        let source = Source::parse(src).unwrap();
        let s = derive(&source, "handler");
        assert_eq!(
            s.statuses.into_iter().collect::<Vec<_>>(),
            [401, 403, 413],
            "202 is a success, not an error"
        );
        assert_eq!(
            s.request_headers.into_iter().collect::<Vec<_>>(),
            ["Idempotency-Key"]
        );
        assert_eq!(
            s.response_headers.into_iter().collect::<Vec<_>>(),
            ["Idempotent-Replayed"]
        );
        assert_eq!(
            derive(&source, "probe"),
            Surface::default(),
            "no credential, no 401"
        );
    }
}
