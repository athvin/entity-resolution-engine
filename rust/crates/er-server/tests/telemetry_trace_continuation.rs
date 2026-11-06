//! The request span is a CONTINUATION of the caller's trace, not a new root.
//!
//! ## What this locks
//!
//! `make_span` builds the span for every non-probe request. It opened a **fresh
//! root** for the whole life of the telemetry work: `info_span!` with no current
//! parent starts a new trace, and nothing read the incoming `traceparent`. The
//! pieces were all present and simply never joined — `er_obs::adopt_traceparent`
//! existed, `acceptable_traceparent` existed, and the *job* path already used
//! both — so click → job → stage → statement was one trace while click → HTTP
//! request was two.
//!
//! A missing call is invisible to every test that asserts on a span's fields,
//! which is why this asserts on the exported span's **lineage** instead: the
//! trace id it carries and whether its parent was remote. Recording the context
//! as a field would satisfy a field assertion and still leave two traces.
//!
//! ## Why the third test is the one that matters
//!
//! `make_span` could have handed the raw header to the propagator and let that
//! judge it. It must not, and the PR that added the hop says so: the span's
//! parent and the value stored on `jobs.trace_context` have to be decided by
//! **one** rule, or a context good enough to parent the span could still be
//! dropped from the job row and the trace would break somewhere other than
//! where it appeared to.
//!
//! A **future-version** context discriminates those two designs, and it is the
//! only shape here that does. `TraceContextPropagator` accepts any version up
//! to `0xfe` on purpose — W3C asks parsers to be forward compatible — while
//! `acceptable_traceparent` refuses everything but `00`, because a `01-`
//! context may carry fields whose meaning we would be guessing at. So the
//! propagator adopts what this server refuses to store, and that gap is what
//! the third test watches.
//!
//! The other refused shapes (uppercase, all-zero ids, version `ff`) are
//! rejected by *both* readings, so they lock the behaviour without being
//! evidence for the design choice. That was worth establishing by experiment
//! rather than assumption: an earlier draft of this file claimed uppercase was
//! the discriminator, and it is not — the propagator checks case itself.

use axum::body::Body;
use axum::http::Request;
use er_obs::testing::capture_spans;
use er_server::telemetry::make_span;

/// The W3C level-1 example context, standing in for one the BFF would mint.
const INCOMING: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const INCOMING_TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

/// Open and close the span `make_span` builds for a `GET /v1/orgs` carrying
/// `traceparent`, and return what was exported.
///
/// No `MatchedPath` extension is set, so the route reads as `<no route>` — not
/// a probe, so the span is real. That is the same shape
/// `a_request_without_a_matched_path_still_produces_a_span` exercises.
fn span_for(traceparent: Option<&str>) -> Vec<er_obs::testing::CapturedSpan> {
    let mut builder = Request::builder().method("GET").uri("/v1/orgs");
    if let Some(value) = traceparent {
        builder = builder.header("traceparent", value);
    }
    let req = builder.body(Body::empty()).expect("building the request");

    capture_spans(|| {
        // Built INSIDE the subscriber, because `info_span!` is only enabled
        // while one is installed — a span made outside would be disabled and
        // would export nothing, which would pass the negative tests for the
        // wrong reason.
        let span = make_span(&req);
        let _entered = span.enter();
    })
}

/// The one span `make_span` produced, failing loudly rather than silently
/// returning nothing — a harness that exports zero spans would make every
/// "is a root" assertion below vacuously true.
fn only_span(spans: &[er_obs::testing::CapturedSpan]) -> &er_obs::testing::CapturedSpan {
    assert_eq!(
        spans.len(),
        1,
        "expected exactly one exported span, got {spans:?}"
    );
    let span = &spans[0];
    // `make_span` sets `otel.name` to `<method> <route>`, which
    // tracing-opentelemetry uses in place of the `info_span!` name — so the
    // exported name is this and not `http.server`. Asserted so that a span
    // from somewhere else could not satisfy the lineage checks below.
    assert_eq!(span.name, "GET <no route>", "the request span's name");
    span
}

#[test]
fn the_request_span_continues_the_callers_trace() {
    let spans = span_for(Some(INCOMING));
    let span = only_span(&spans);

    assert_eq!(
        span.trace_id, INCOMING_TRACE_ID,
        "the request span did not take the caller's trace id, so a click in the \
         browser and the HTTP request it caused are two unrelated traces"
    );
    assert!(
        span.parent_is_remote,
        "the parent is not marked remote, so this span is under a local parent \
         rather than continuing the caller's context"
    );
    assert!(!span.is_root, "a continuation must not be a root");
}

#[test]
fn without_a_traceparent_the_request_span_is_a_root() {
    // The control, and also the permanent correct answer for a direct API
    // client, a probe or a curl. Without it the test above could pass because
    // the id appeared incidentally rather than because adoption worked.
    let spans = span_for(None);
    let span = only_span(&spans);

    assert!(
        span.is_root,
        "a request carrying no traceparent must start its own trace"
    );
    assert!(
        !span.parent_is_remote,
        "there was no remote context to adopt"
    );
    assert_ne!(
        span.trace_id, INCOMING_TRACE_ID,
        "the caller's trace id appeared without a traceparent header, so the \
         positive test proves nothing"
    );
}

#[test]
fn a_future_version_context_is_not_adopted() {
    // The discriminating case. `01-` is well formed and the propagator will
    // happily adopt it — W3C asks parsers to accept any version up to `0xfe` —
    // but `acceptable_traceparent` refuses it, so it is never stored on
    // `jobs.trace_context`. If this span takes it as a parent, the span's
    // reading of the header and the job row's reading have diverged, and a
    // trace will break somewhere other than where anybody is looking.
    //
    // Verified to discriminate: replacing the validated call in `make_span`
    // with the raw header fails exactly this assertion.
    let future = "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    let spans = span_for(Some(future));
    let span = only_span(&spans);

    assert!(
        span.is_root,
        "a traceparent this server refuses to STORE was adopted as the span's \
         parent anyway — the two readings of the header are no longer one rule"
    );
    assert_ne!(
        span.trace_id, INCOMING_TRACE_ID,
        "the refused context's trace id reached the span anyway"
    );
}

#[test]
fn a_malformed_traceparent_does_not_fail_the_request() {
    // A bad correlation handle must never fail an otherwise valid request, so
    // each of these must produce a span rather than a panic. They are the
    // shapes `acceptable_traceparent`'s own unit tests cover, re-run here
    // through the real header path.
    //
    // Both readings of the header refuse all of these, so unlike the test
    // above they are not evidence about *which* rule is being applied — they
    // lock that a malformed value is ignored rather than fatal.
    for hostile in [
        "",
        "   ",
        "not-a-traceparent",
        "00-tooshort-0-1",
        // All-zero trace id: the spec calls it invalid.
        "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
        // All-zero span id, likewise.
        "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
        // Version `ff` is forbidden outright.
        "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        // Uppercase: the spec requires lowercase, and accepting mixed case
        // would let one context arrive as two distinct strings.
        "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
    ] {
        let spans = span_for(Some(hostile));
        let span = only_span(&spans);
        assert!(
            span.is_root,
            "{hostile:?} must leave the span a root, not parent it"
        );
    }
}
