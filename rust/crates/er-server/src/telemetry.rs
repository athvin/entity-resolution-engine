//! The HTTP layer's telemetry: a span per request, and a correlation id.
//!
//! ## Where these layers sit, and why it is not obvious
//!
//! [`crate::app`] wraps `build_routes` in an **outer** service that rewrites
//! the BFF's `{id}:action` spelling to the `{id}/action` sub-path the router
//! registers. That rewrite has to happen *before* routing, because an unknown
//! `:action` path matches no route at all — so the outer service hands the
//! request away by value and cannot see `MatchedPath`.
//!
//! `MatchedPath` is what makes a span name a bounded *route template*
//! (`GET /v1/orgs/{org}/jobs`) rather than an unbounded populated path
//! (`GET /v1/orgs/acme/jobs`), and the difference is the §10.3 cardinality rule.
//! axum inserts it before the route service runs, and `Router::layer` applies
//! to the path router, the fallback router and the catch-all fallback — so a
//! layer added there sees it, including on a 404.
//!
//! So: **the normalizer stays outside and keeps rewriting URIs; the trace layer
//! goes inside `build_routes`, where routing has already happened.** Request-id
//! minting goes outermost, so even a request that routes nowhere carries one.
//!
//! ## The request span is a CONTINUATION, not a root
//!
//! [`make_span`] adopts the caller's `traceparent`, so a click in the browser,
//! the BFF hop, this request and every statement it executes are **one trace**.
//!
//! It did not, until this was found by querying real exported spans: the P7
//! action header was present on a `GET /v1/orgs` span and the trace id of the
//! request that caused it was nowhere, because `info_span!` with no current
//! parent starts a fresh root and nothing here ever read the header. The hop
//! existed on the *job* path only — `jobs.trace_context` stores the traceparent
//! and `dispatcher.rs` adopts it — so click → job → stage → statement joined up
//! while click → HTTP request did not. Every read the UI makes was its own
//! unrelated trace, which is most of them.
//!
//! `er_obs::propagate`'s own module doc names "the HTTP request that carried
//! it" as part of the one trace it exists to build, so this was a missing call
//! rather than a deliberate boundary.
//!
//! ## What is deliberately absent
//!
//! No metric instruments *here*. `er_obs::metrics` owns the instruments and the
//! registry that bounds their cardinality (one `er.db.query.duration` histogram
//! over two closed dimensions); a second, hand-rolled instrument in this layer
//! would be a cardinality decision made in the wrong place. RED metrics over
//! `http.route` derive from these spans through a `spanmetrics` connector in the
//! collector's config, which is the cheaper place to put a decision that may
//! need reverting.

use axum::body::Body;
use axum::extract::MatchedPath;
use axum::http::{Request, Response};
use std::time::Duration;
use tracing::Span;

/// The correlation id header. Accepted from the caller when it sends one — the
/// BFF's id should win, so one request has one id end to end — and minted here
/// when it does not.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// The header naming the UI control that caused this request.
///
/// Sent by the BFF, which read it from the browser and validated it against the
/// closed `UI_ACTIONS` vocabulary in `frontend/src/lib/telemetry/actions.ts`.
pub const ACTION_HEADER: &str = "x-er-action";

/// Longest action name recorded. The vocabulary's longest member is well under
/// this; the bound exists for values that did not come from the vocabulary.
const ACTION_MAX: usize = 48;

/// Whether an `x-er-action` value is fit to record.
///
/// **Shape, not membership.** The authoritative list is TypeScript, and
/// duplicating it here would create a two-language vocabulary that drifts
/// silently in whichever direction nobody is testing. The real guarantee is
/// `frontend/scripts/check-er-actions.mjs`, which fails the build on an
/// undeclared name; this is the untrusted-input bound underneath it.
///
/// Shape is enough for what it has to prevent. The rule — lowercase, digits,
/// dots and underscores only, at least one dot, no leading or trailing dot —
/// bounds span cardinality and refuses the payload shapes that matter here: an
/// email carries `@`, a record key carries uppercase or hyphens, a query string
/// carries `=` and `&`. All are rejected.
///
/// Split from the extractor so the rule is testable without a request, the same
/// reason [`is_probe`] is split from [`make_span`].
#[must_use]
pub fn acceptable_action(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= ACTION_MAX
        && raw.contains('.')
        && !raw.starts_with('.')
        && !raw.ends_with('.')
        && raw
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
}

/// Whether a route is a Kubernetes probe, and therefore not worth a span.
///
/// The chart probes `/readyz` every 5 s for readiness and `/healthz` every 10 s
/// for liveness, which at one replica is roughly 780k spans a month carrying no
/// information. Dropping them here rather than in the collector means we never
/// pay to ship them. (`/readyz` is also the startup probe, every 3 s, but only
/// until the pod starts — a bounded burst, not a rate.) Both paths have been in
/// this match since before the chart read either one, which is why swapping
/// which probe reads which needed no change here.
///
/// Split out from [`make_span`] so the rule is testable without a request —
/// `MatchedPath` has no public constructor, so a unit test cannot build the
/// input `make_span` reads. Same reason `acceptable_actor` is split from
/// `forwarded_actor`.
#[must_use]
pub fn is_probe(route: &str) -> bool {
    matches!(route, "/healthz" | "/readyz")
}

/// Build the span for one request. [`Span::none`] for a probe — see [`is_probe`].
///
/// The returned span is a **child of the caller's trace** when the request
/// carried an acceptable `traceparent`; see the module doc for what was broken
/// before it was. A fresh root is the correct and permanent answer for a caller
/// that sent none — a direct API client, a probe, a curl.
pub fn make_span(req: &Request<Body>) -> Span {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or("<no route>", MatchedPath::as_str);

    if is_probe(route) {
        return Span::none();
    }

    let request_id = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // The control the person pressed. Without it a trace says a job ran and
    // every statement it executed, and not which button asked for it — and a
    // reader would have to walk to the root span to find out. Empty for a
    // request no control caused, which is most of them.
    let action = req
        .headers()
        .get(ACTION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| acceptable_action(v))
        .unwrap_or("");

    // `url.path` is recorded; `url.query` is NOT, and that is the §10.4 rule
    // rather than an oversight. `GET /v1/orgs/{org}/golden-records` takes `q`,
    // a contains-search over record values, so the query string is payload in
    // the one sense this engine must never log. The path carries identifiers.
    let span = tracing::info_span!(
        "http.server",
        otel.name = %format_args!("{} {}", req.method(), route),
        otel.kind = "server",
        http.request.method = %req.method(),
        http.route = %route,
        url.path = %req.uri().path(),
        er.request_id = %request_id,
        // Span-only, never a metric dimension: the vocabulary grows with the
        // UI, and a per-action metric series is the cardinality mistake §10
        // already priced at ~$131/month for one histogram. A per-action
        // aggregate, if ever wanted, is a `spanmetrics` connector in the
        // collector's values file.
        er.action = %action,
        // Recorded by the bearer guard, which is the first place the credential
        // has been resolved to an org and a role.
        er.org = tracing::field::Empty,
        er.actor = tracing::field::Empty,
        er.role = tracing::field::Empty,
        http.response.status_code = tracing::field::Empty,
    );

    // The hop. Validated with the same rule that decides whether a traceparent
    // is fit to STORE on `jobs.trace_context`, deliberately rather than letting
    // the propagator judge it: the two must agree, or a context good enough to
    // parent this span could still be dropped from the job row, and the trace
    // would then break at a different place than the one anybody is looking at.
    //
    // A rejected or absent header leaves the fresh root alone. That is the same
    // choice `forwarded_traceparent` makes and for the same reason — a bad
    // correlation handle must never fail an otherwise valid request.
    if let Some(parent) = crate::forwarded_traceparent_header(req.headers()) {
        er_obs::adopt_traceparent(&span, &parent);
    }

    span
}

/// Record the outcome on the span and emit the request log line.
///
/// **Returns early for a probe.** A disabled span does not suppress an *event*,
/// so without this check every `/healthz` still wrote a log line — which is the
/// expensive half, since CloudWatch bills by the byte and the span was never
/// going anywhere. Found by running it, not by reading it.
pub fn on_response(res: &Response<Body>, latency: Duration, span: &Span) {
    if span.is_disabled() {
        return;
    }

    let status = res.status();
    span.record("http.response.status_code", status.as_u16());

    // A 5xx is ours; a 4xx is the caller's. Logging them at one level would
    // bury the former in the latter, and the read path produces a steady
    // trickle of 4xx by design — a cross-org read is a 404, never a 403, so
    // "not found" is the normal answer to a legitimate question.
    let micros = latency.as_micros();
    if status.is_server_error() {
        tracing::error!(
            status = status.as_u16(),
            duration_us = micros,
            "request failed"
        );
    } else if status.is_client_error() {
        tracing::debug!(
            status = status.as_u16(),
            duration_us = micros,
            "request refused"
        );
    } else {
        tracing::info!(status = status.as_u16(), duration_us = micros, "request");
    }
}

/// Record a failure that produced **no response** — a dropped connection, a
/// body that never finished.
///
/// `TraceLayer::new_for_http` classifies a 5xx as a failure too, so matching
/// only the `Error` variant is what stops every 5xx being logged twice: once
/// here and once in [`on_response`], which already reports it with the route
/// attached. The status arm is deliberately ignored rather than unreachable.
pub fn on_failure(
    class: tower_http::classify::ServerErrorsFailureClass,
    latency: Duration,
    span: &Span,
) {
    use tower_http::classify::ServerErrorsFailureClass as FailureClass;
    if span.is_disabled() {
        return;
    }
    match class {
        // Already reported by `on_response`, with more context than is
        // available here.
        FailureClass::StatusCode(_) => {}
        FailureClass::Error(error) => tracing::error!(
            error = %error,
            duration_us = latency.as_micros(),
            "request did not complete"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{ACTION_MAX, acceptable_action, is_probe, make_span};
    use axum::body::Body;

    #[test]
    fn only_the_probes_are_unspanned() {
        // ~780k spans a month of nothing, at one replica, if the left column
        // regresses. The right column is the cost of being too clever with a
        // prefix match: these must all still be traced.
        for probe in ["/healthz", "/readyz"] {
            assert!(is_probe(probe), "{probe} must not be spanned");
        }
        for real in [
            "/v1/orgs",
            "/v1/orgs/{org}/jobs",
            "/v1/orgs/{org}/golden-records",
            "/healthzz",
            "/readyz/sub",
            "<no route>",
        ] {
            assert!(!is_probe(real), "{real} must be traced");
        }
    }

    #[test]
    fn a_request_without_a_matched_path_still_produces_a_span() {
        // A 404 reaches the catch-all fallback with no `MatchedPath`. axum's
        // `Router::layer` applies there too, so this path is real — and it must
        // not panic or be mistaken for a probe.
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/nope")
            .body(Body::empty())
            .unwrap();
        drop(make_span(&req));
    }

    #[test]
    fn a_declared_action_is_accepted() {
        for action in [
            "run.start",
            "review.decide.match",
            "review.bulk.no_match",
            "label.skip",
            "records.merge.confirm",
            "dataset.create",
        ] {
            assert!(acceptable_action(action), "{action} should be accepted");
        }
    }

    #[test]
    fn the_payload_shapes_that_matter_here_are_refused() {
        // This is the point of the function. The browser only sends declared
        // names and a build gate enforces that, but the header is untrusted,
        // and in a deduplication engine the things that could arrive instead
        // are exactly the record values. Each of these is a real string from
        // this app's own markup or routes.
        for hostile in [
            "member-remove-alice@acme.com", // the data-testid that caused the PII fix
            "Remove alice@acme.com",        // the aria-label on the same button
            "mark-CRM-20481",               // a record key
            "q=jane+smith&org=acme",        // a query string, which is payload here
            "../../etc/passwd",
            "run.start\nx-injected: 1", // a header-splitting attempt
            "RUN.START",                // uppercase is not in the vocabulary's shape
        ] {
            assert!(!acceptable_action(hostile), "{hostile:?} should be refused");
        }
    }

    #[test]
    fn shape_edges_are_refused() {
        assert!(!acceptable_action(""), "empty");
        assert!(!acceptable_action("nodot"), "a name with no dot is not one");
        assert!(!acceptable_action(".leading"), "leading dot");
        assert!(!acceptable_action("trailing."), "trailing dot");
        assert!(
            !acceptable_action(&format!("a.{}", "x".repeat(ACTION_MAX))),
            "over the length bound"
        );
    }
}
