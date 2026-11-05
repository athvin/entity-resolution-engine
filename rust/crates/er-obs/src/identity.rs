//! Reading the ambient trace id and session, for the rows that outlive a span.
//!
//! ## The problem this solves
//!
//! Spans answer "what did this request do" beautifully and then expire — ours
//! land in OpenObserve on a retention we choose, and CloudWatch's vended span
//! store keeps 30 days. The audit ledger is the opposite: it is a compliance
//! artifact on a 400-day tier that must still answer "what did this person do"
//! long after every span of that session is gone.
//!
//! Writing the trace id onto the audit row is what joins the two while both
//! exist, and what leaves a usable record once only one does. It turns "which
//! rows did this session write" into "and here is every SQL statement each of
//! them ran", which is the question the whole exercise is for.
//!
//! ## Why this is read ambiently instead of threaded
//!
//! `pg::audit` has 25 call sites. The obvious design — a second
//! `audit_with(…, trace_id, session_id)` and a migration of each site as it is
//! touched — would leave the column mostly NULL for as long as the migration
//! took, and a mostly-NULL column reads as "this request had no trace" rather
//! than "nobody has got to this call site yet".
//!
//! `tracing` span context is already ambient, so there is nothing to thread:
//! every one of those 25 sites is called inside the request span that
//! `er-server`'s HTTP layer opened, and that span knows its own trace id. All
//! 25 are therefore correct at once, and a 26th is correct the day it is
//! written.
//!
//! ## Why there is no session id here
//!
//! `audit_log` deliberately carries no `session_id`, and this module offers no
//! way to read one. er-server has no trustworthy source for it: the only way it
//! could learn a session is a header, and `forwarded_actor`'s own doc comment
//! records that `x-acting-user` is **spoofable by any API-key holder by design**
//! — a sibling `x-er-session` would inherit that exactly.
//!
//! Writing a spoofable value into a 400-day compliance ledger, beside a column
//! named `actor` that readers will reasonably assume is authoritative, is worse
//! than leaving it out. So the session is recorded once, in `erweb.audit`, by
//! the BFF that reads it server-side from the httpOnly cookie — and `trace_id`
//! is the key that joins the two trails. That also matches where each trail is
//! already strongest: the BFF knows who the person is, er-server knows what the
//! statement did.
//!
//! ## What is NOT recorded, and why
//!
//! The actor's **email** never reaches a span or this module, per §10.4. It is
//! already on the audit row in plaintext, because that row is in-account
//! Postgres on a compliance tier; a span's boundary is an exportable store.

use opentelemetry::trace::TraceContextExt;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// The current trace id as 32 lowercase hex, or `None` when there is no
/// recording span.
///
/// `None` is a normal answer and a permanent one for whole classes of caller:
/// the pipeline CLI, the dispatcher's own bookkeeping, a test. It means "this
/// did not happen inside a traced request", which is true and worth recording
/// as NULL rather than papering over with a synthetic id.
///
/// The returned value is the bare trace id, not a `traceparent`. The parent span
/// id in a traceparent is per-hop, so storing the whole thing would produce a
/// string that cannot be joined against the span store without parsing.
#[must_use]
pub fn current_trace_id() -> Option<String> {
    let context = tracing::Span::current().context();
    let span_context = context.span().span_context().clone();
    // An invalid context is the no-subscriber and no-exporter case. Both yield
    // the all-zero trace id, which must not be written as though it were real.
    if !span_context.is_valid() {
        return None;
    }
    Some(span_context.trace_id().to_string())
}

#[cfg(test)]
mod tests {
    use super::current_trace_id;
    use opentelemetry::trace::TraceContextExt as _;

    #[test]
    fn outside_a_traced_request_it_is_none() {
        // The permanent case for the CLI and the dispatcher's own bookkeeping,
        // and the state every test binary runs in. Must not panic and must not
        // invent an id.
        assert_eq!(current_trace_id(), None);
    }

    /// Inside a real recording span, the id is that span's own.
    ///
    /// The three `None` cases above only prove this does not lie when there is
    /// nothing to report. This is the one that proves it reports the RIGHT
    /// thing — without it, a function that always returned `None` would pass
    /// every other test in the file.
    ///
    /// Uses a scoped subscriber rather than a global one: `er-obs`'s own tests
    /// run as threads in one process, so installing a process-global subscriber
    /// here would poison every sibling test.
    #[test]
    fn inside_a_recording_span_it_is_that_spans_trace_id() {
        use opentelemetry::trace::TracerProvider as _;
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        use tracing_subscriber::layer::SubscriberExt;

        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("test"));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("http.server");
            let _entered = span.enter();

            let observed = current_trace_id().expect("a recording span has a trace id");
            // Compared against what the span itself reports, not a literal: a
            // hard-coded id would only prove the function returns a constant.
            let expected = span.context().span().span_context().trace_id().to_string();

            assert_eq!(observed, expected);
            assert_eq!(observed.len(), 32, "32 lowercase hex, joinable as-is");
            assert!(
                observed
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            );
            assert_ne!(observed, "0".repeat(32));
        });
    }

    #[test]
    fn the_all_zero_trace_id_is_reported_as_absent() {
        // With no exporter installed the context is invalid and its trace id is
        // all zeroes. Writing that onto an audit row would be worse than NULL —
        // every untraced row would share one plausible-looking id and appear to
        // be the same request.
        let span = tracing::info_span!("http.server");
        let _entered = span.enter();
        assert_eq!(current_trace_id(), None);
    }
}
