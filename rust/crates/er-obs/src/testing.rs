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

fn to_captured(span: opentelemetry_sdk::trace::SpanData) -> CapturedSpan {
    CapturedSpan {
        name: span.name.to_string(),
        trace_id: span.span_context.trace_id().to_string(),
        parent_span_id: span.parent_span_id.to_string(),
        parent_is_remote: span.parent_span_is_remote,
        is_root: span.parent_span_id == SpanId::INVALID,
    }
}

/// A handle to the process-global recording subscriber's captured spans.
///
/// Clone freely — the exporter is shared. Every clone sees every span the
/// process has finished since installation, including other tests', so
/// assertions should select by the span they mean (a trace id, a name).
#[derive(Debug, Clone)]
pub struct GlobalRecorder {
    exporter: InMemorySpanExporter,
}

impl GlobalRecorder {
    /// Every span finished so far, in close order.
    #[must_use]
    pub fn captured(&self) -> Vec<CapturedSpan> {
        self.exporter
            .get_finished_spans()
            .unwrap_or_default()
            .into_iter()
            .map(to_captured)
            .collect()
    }
}

/// Install the process-global recording subscriber and return its handle.
///
/// This exists because [`capture_spans`]'s thread-local subscriber composes
/// badly with a *concurrent* test binary. Callsite interest is cached
/// process-wide, and a callsite first hit on an unsubscribed thread caches
/// "never interested" for the whole process — including later hits on the
/// subscribed thread. Observed on PR #841: the same test passed alone and
/// failed under the pg shard with zero spans captured, flapping on which
/// thread hit each callsite first. A global default closes the race: once
/// set, no thread ever evaluates the none dispatch again, so no callsite can
/// cache "never".
///
/// The subscriber is installed once per process and lives to process end;
/// every caller gets a handle to the same exporter.
pub fn global_recorder() -> GlobalRecorder {
    static GLOBAL: std::sync::OnceLock<GlobalRecorder> = std::sync::OnceLock::new();
    GLOBAL
        .get_or_init(|| {
            let exporter = InMemorySpanExporter::default();
            let provider = SdkTracerProvider::builder()
                .with_sampler(Sampler::AlwaysOn)
                .with_simple_exporter(exporter.clone())
                .build();
            let subscriber = tracing_subscriber::registry().with(
                tracing_opentelemetry::layer()
                    .with_tracer(provider.tracer("er-obs::testing::global"))
                    .with_filter(crate::SpansOnly),
            );
            tracing::subscriber::set_global_default(subscriber)
                .expect("a global default subscriber is already installed");
            // A callsite first hit before this point may have cached "never
            // interested" from the no-subscriber dispatch; clear the cache so
            // every callsite re-registers against this subscriber.
            tracing::callsite::rebuild_interest_cache();
            // The provider must outlive every span its tracers record; the
            // process ends first.
            std::mem::forget(provider);
            GlobalRecorder { exporter }
        })
        .clone()
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
        .map(to_captured)
        .collect()
}
