//! Adopting a trace context that arrived as a string.
//!
//! ## The hop this closes
//!
//! `jobs.trace_context` has carried the traceparent of the request that
//! enqueued a job since P3, and the dispatcher has recorded it as a span
//! *field* ever since. A field joins two traces by log search; it does not make
//! them one trace. `dispatcher.rs` said so in a comment — "making this span a
//! true CHILD of that context needs the OTel context propagator, which arrives
//! with the exporter — at which point this field becomes `set_parent`" — and
//! the exporter has now arrived.
//!
//! So a click that enqueues a job, the HTTP request that carried it, the
//! dispatcher that claimed it and every statement the run executes become one
//! trace, which is the thing the whole exercise is for.
//!
//! ## Why this lives in er-obs and not er-server
//!
//! The propagator is part of the exporter's world: it needs
//! `opentelemetry_sdk` and `tracing-opentelemetry`, and `er-server` has
//! neither. Keeping it here means the control plane gains a one-line call
//! rather than two dependencies, and the OTel surface stays in the one crate
//! that already owns it.

use std::collections::HashMap;

use opentelemetry::propagation::TextMapPropagator;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Make `span` a child of the W3C context in `traceparent`.
///
/// Best effort and silent on failure, deliberately. A context that will not
/// parse means the span stays a root — which is exactly what it was before this
/// function existed — and a job must never fail because its telemetry lineage
/// could not be established. `er_server::acceptable_traceparent` has already
/// refused anything malformed on the way in, so a value reaching here is
/// well-formed or absent.
///
/// A blank or absent context is the normal case for a job enqueued by the
/// scheduler rather than by a request, and it is not worth a log line on every
/// claim.
pub fn adopt_traceparent(span: &tracing::Span, traceparent: &str) {
    let traceparent = traceparent.trim();
    if traceparent.is_empty() {
        return;
    }

    // The propagator reads from a generic carrier; a one-entry map is the
    // smallest thing that satisfies it.
    let mut carrier = HashMap::new();
    carrier.insert("traceparent".to_string(), traceparent.to_string());

    let context = TraceContextPropagator::new().extract(&carrier);

    // `set_parent` fails when the span is disabled or already has a parent.
    // Both are fine: a disabled span is not exported, and a span that already
    // has a parent has a better one than this.
    if span.set_parent(context).is_err() {
        tracing::trace!("span already had a parent; keeping it");
    }
}

#[cfg(test)]
mod tests {
    use super::adopt_traceparent;

    #[test]
    fn a_blank_or_absent_context_is_a_no_op() {
        // The scheduler enqueues jobs with no request behind them, so this is
        // the common path rather than an error path. It must not panic and must
        // not need a subscriber.
        let span = tracing::info_span!("job");
        adopt_traceparent(&span, "");
        adopt_traceparent(&span, "   ");
    }

    #[test]
    fn a_malformed_context_leaves_the_span_a_root() {
        // Reaching this would mean `acceptable_traceparent` let something
        // through, but the function still must not panic on it: a job failing
        // because its lineage could not be parsed would be the worst possible
        // trade.
        let span = tracing::info_span!("job");
        adopt_traceparent(&span, "not-a-traceparent");
        adopt_traceparent(&span, "00-tooshort-0-1");
    }

    #[test]
    fn a_valid_context_is_accepted_without_a_subscriber_installed() {
        // With no subscriber the span is disabled and `set_parent` errors;
        // swallowing that is the point, since every test binary in the
        // workspace runs in exactly this state.
        let span = tracing::info_span!("job");
        adopt_traceparent(
            &span,
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        );
    }
}
