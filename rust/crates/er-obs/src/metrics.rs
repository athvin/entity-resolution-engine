//! The meter, and the one aggregate histogram.
//!
//! ## Where the measurement comes from, and why no new plumbing was needed
//!
//! The plan called for a `fn`-pointer `OnceLock` sink in `er-span` that this
//! module would fill in during `init`, so generated code could reach a meter
//! without `er-span` depending on `er-obs` (which would cycle, and would pull
//! `opentelemetry_sdk` into the compile of 42 generated files).
//!
//! That mechanism turned out to be unnecessary. [`crate::executions`] is
//! already a `tracing_subscriber::Layer` installed on every process, and by the
//! time it sees `on_close` it holds exactly the three things a histogram needs:
//! the store from `db.system.name`, the outcome from `er.outcome`, and the
//! elapsed time from the span's own lifetime. So the metric is recorded there,
//! `er-span` keeps its single `tracing` dependency, and there is no inverted
//! call direction to reason about.
//!
//! This is the same realisation the plan reached for the per-execution fact
//! table — *"the mechanism is a `Layer`, not a call from generated code"* —
//! applied one layer further out.
//!
//! ## Why the histogram is aggregate
//!
//! One `er.db.query.duration` dimensioned by store and outcome: four series.
//! The arithmetic ruling out `statement_name` as a dimension is in
//! [`crate::registry`]; the short version is ~$148/month for one histogram.
//! Statement identity lives on the span, where it is free and groupable.

use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::metrics::{Histogram, Meter};
use opentelemetry::{KeyValue, global};

/// The instruments, built once when a meter provider is available.
struct Instruments {
    query_duration: Histogram<f64>,
}

static INSTRUMENTS: OnceLock<Instruments> = OnceLock::new();

/// Build the instruments from `meter`.
///
/// Idempotent: a second call is a no-op, which matters because
/// `er-server/tests/main.rs` runs ~50 modules as threads in one process and any
/// of them may reach initialisation.
pub(crate) fn install(meter: &Meter) {
    // `let _` because losing the race is the expected case for a second
    // caller, not an error worth reporting.
    let _ = INSTRUMENTS.set(Instruments {
        query_duration: meter
            .f64_histogram("er.db.query.duration")
            .with_unit("ms")
            .with_description("How long a SQL statement took, across every statement.")
            .build(),
    });
}

/// Install from the global meter provider.
///
/// Split from [`install`] so a test can supply its own meter with no exporter
/// and no network.
pub(crate) fn install_from_global() {
    install(&global::meter_provider().meter("er-obs"));
}

/// Record one statement's duration.
///
/// Called from [`crate::executions`]'s `on_close`. Total by construction: an
/// absent provider, an unarmed instrument or a span missing its dimensions all
/// end here as a no-op. Observation must never change a run — the contract
/// `reporter.rs` states — and a metric that cannot be recorded is not a reason
/// to fail a statement.
///
/// A span missing `db.system.name` or `er.outcome` is **skipped rather than
/// defaulted**. Recording it as `unknown` would add a fifth series that exists
/// only to say a span was malformed, and would quietly absorb exactly the
/// spans worth noticing.
pub(crate) fn record_query_duration(store: Option<&str>, outcome: Option<&str>, elapsed: Duration) {
    let Some(instruments) = INSTRUMENTS.get() else {
        return;
    };
    let (Some(store), Some(outcome)) = (store, outcome) else {
        return;
    };
    instruments.query_duration.record(
        // Milliseconds as f64, not u64: a sub-millisecond control-plane
        // statement is common, and truncating it to 0 would make the fast half
        // of the distribution invisible.
        elapsed.as_secs_f64() * 1000.0,
        &[
            // The ONLY two dimensions, both closed: two stores, two outcomes,
            // four series. `tests/cardinality.rs` holds the line.
            KeyValue::new("db.system.name", store.to_owned()),
            KeyValue::new("outcome", outcome.to_owned()),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_without_a_provider_is_a_no_op() {
        // The state every test binary and the `er` CLI run in. Must not panic:
        // a metric that cannot be recorded must never fail a statement.
        record_query_duration(Some("duckdb"), Some("ok"), Duration::from_millis(5));
        record_query_duration(
            Some("postgresql"),
            Some("error"),
            Duration::from_micros(300),
        );
    }

    #[test]
    fn a_span_missing_its_dimensions_is_skipped_not_defaulted() {
        // Must not panic, and must not invent a dimension value — see the
        // function's docs for why `unknown` would be worse than nothing.
        record_query_duration(None, Some("ok"), Duration::from_millis(1));
        record_query_duration(Some("duckdb"), None, Duration::from_millis(1));
        record_query_duration(None, None, Duration::from_millis(1));
    }

    #[test]
    fn the_emission_site_matches_what_the_registry_declares() {
        // Guards the easy mistake: adding `er.statement` as a dimension because
        // it is right there in scope. `tests/cardinality.rs` checks the
        // registry's claim; this checks the registry still describes the code.
        let declared = crate::registry::METRICS
            .iter()
            .find(|m| m.name == "er.db.query.duration")
            .expect("the histogram is declared in the registry");
        assert_eq!(declared.dimensions, &["db.system.name", "outcome"]);
        assert_eq!(declared.series, 4);
        assert_eq!(declared.unit, "ms");
    }
}
