//! A span-capturing subscriber, for the tests that must prove a span's lineage.
//!
//! ## Why this harness lives here rather than beside the code under test
//!
//! This crate's `Cargo.toml` is the **one** manifest in the workspace that
//! names the `opentelemetry*` versions, and it names them as a group that has
//! to move in a single commit: the four `opentelemetry*` crates exchange types
//! with each other while `tracing-opentelemetry` sits exactly one minor ahead.
//! A test in another crate that reached for `tracing-opentelemetry` itself
//! would make a second place to bump, and the failure mode of getting that
//! wrong is a trait that is not implemented for a type of the same name —
//! which reads as nonsense at the call site rather than as a version skew.
//!
//! So the harness is exported from here and its callers need no OTel
//! dependency of their own. That is the same reasoning that put
//! [`adopt_traceparent`](crate::adopt_traceparent) in this crate rather than
//! in `er-server`: the OTel surface stays in the one crate that already owns
//! it, and the control plane gains a call rather than two dependencies.
//!
//! ## Why it reports plain strings and not `SpanData`
//!
//! Handing back `opentelemetry_sdk`'s own type would put that crate straight
//! back into the caller's `use` lines and defeat the paragraph above. The
//! reduction is deliberate: a lineage assertion needs the ids, the name and
//! whether the parent was remote, and nothing else.
//!
//! The ids are read from the SDK's own `span_context` rather than sniffed out
//! of an encoded payload, so formatting them as hex here is safe. The sibling
//! `tests/adopts_parent_context.rs` asserts on raw OTLP bytes instead, because
//! it is checking what reaches the wire; that is a different question and its
//! own doc comment explains why a hex string would have been the wrong
//! assertion there.

use opentelemetry::trace::{SpanId, TracerProvider as _};
use opentelemetry_sdk::trace::{InMemorySpanExporter, Sampler, SdkTracerProvider};
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;

/// One exported span, reduced to what a lineage assertion needs.
#[derive(Debug, Clone)]
pub struct CapturedSpan {
    /// The span's name — `http.server`, `job`, and so on.
    pub name: String,
    /// The trace id, 32 lowercase hex.
    pub trace_id: String,
    /// The parent span id, 16 lowercase hex; all zeroes when there is none.
    pub parent_span_id: String,
    /// Whether the parent arrived in an **extracted** context rather than
    /// being a span of this process.
    ///
    /// This is the field that separates "continued a caller's trace" from
    /// "happens to sit under a local parent", so it is the one a propagation
    /// test should assert.
    pub parent_is_remote: bool,
    /// Whether this span began a trace rather than continuing one.
    pub is_root: bool,
}

/// Run `f` with a span-exporting subscriber installed, and return every span
/// it finished.
///
/// `Sampler::AlwaysOn` is set explicitly, for the reason
/// [`crate::provider_for`] sets it: the builder's default is `ParentBased`,
/// which honours a remote parent's sampled flag. A test that adopted a
/// `…-00` context under the default would capture nothing and read as a
/// lineage failure rather than as a sampling decision.
///
/// That is measured, not assumed — removing this line and capturing a span
/// under an unsampled remote parent yields zero spans, and restoring it yields
/// one. `tests/adopts_parent_context.rs` asserts the same property against the
/// real `provider_for`, which is where it actually has to hold.
///
/// Spans are exported on close, so `f` must both open and **close** the spans
/// it cares about — dropping the guard, or letting it fall out of scope at the
/// end of the closure, is enough.
#[must_use]
pub fn capture_spans<F: FnOnce()>(f: F) -> Vec<CapturedSpan> {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .with_simple_exporter(exporter.clone())
        .build();

    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("er-obs::testing"))
            // Spans leave the process; events do not. The same filter the real
            // subscriber installs, so what is captured here is what would be
            // exported in production.
            .with_filter(crate::SpansOnly),
    );

    tracing::subscriber::with_default(subscriber, f);

    // Best effort: a simple exporter has already written each span on close, so
    // a flush failure cannot lose one. Swallowed rather than unwrapped because
    // a harness that panics on teardown masks the assertion it was built for.
    let _ = provider.force_flush();

    exporter
        .get_finished_spans()
        .unwrap_or_default()
        .into_iter()
        .map(|span| CapturedSpan {
            name: span.name.to_string(),
            trace_id: span.span_context.trace_id().to_string(),
            parent_span_id: span.parent_span_id.to_string(),
            parent_is_remote: span.parent_span_is_remote,
            is_root: span.parent_span_id == SpanId::INVALID,
        })
        .collect()
}
