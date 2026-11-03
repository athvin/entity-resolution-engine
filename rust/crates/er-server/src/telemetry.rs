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
//! ## What is deliberately absent
//!
//! No metric instruments. Spans now export over OTLP (see `er_obs::otlp`) but
//! the collector's `metrics` pipeline still has no producer, so a histogram here
//! would have nowhere to go — and the cardinality decision it would commit to
//! is the one worth deferring longest. The spans carry the latency and the
//! status in the meantime, which the subscriber writes as structured records —
//! and RED metrics derived from spans are a `spanmetrics` connector in the
//! collector's config rather than Rust code, which is the cheaper place to put
//! a cardinality decision that may need reverting.

use axum::body::Body;
use axum::extract::MatchedPath;
use axum::http::{Request, Response};
use std::time::Duration;
use tracing::Span;

/// The correlation id header. Accepted from the caller when it sends one — the
/// BFF's id should win, so one request has one id end to end — and minted here
/// when it does not.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Whether a route is a Kubernetes probe, and therefore not worth a span.
///
/// The chart probes `/healthz` every 5 s for readiness and every 10 s for
/// liveness, which at one replica is roughly 780k spans a month carrying no
/// information. Dropping them here rather than in the collector means we never
/// pay to ship them.
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

    // `url.path` is recorded; `url.query` is NOT, and that is the §10.4 rule
    // rather than an oversight. `GET /v1/orgs/{org}/golden-records` takes `q`,
    // a contains-search over record values, so the query string is payload in
    // the one sense this engine must never log. The path carries identifiers.
    tracing::info_span!(
        "http.server",
        otel.name = %format_args!("{} {}", req.method(), route),
        otel.kind = "server",
        http.request.method = %req.method(),
        http.route = %route,
        url.path = %req.uri().path(),
        er.request_id = %request_id,
        // Recorded by the bearer guard, which is the first place the credential
        // has been resolved to an org and a role.
        er.org = tracing::field::Empty,
        er.actor = tracing::field::Empty,
        er.role = tracing::field::Empty,
        http.response.status_code = tracing::field::Empty,
    )
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
    use super::{is_probe, make_span};
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
}
