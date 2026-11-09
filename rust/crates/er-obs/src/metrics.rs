//! The meter, and the one aggregate histogram.
//!
//! ## Where the measurement comes from, and why no new plumbing was needed
//!
//! The plan called for a `fn`-pointer `OnceLock` sink in `er-span` that this
//! module would fill in during `init`, so generated code could reach a meter
//! without `er-span` depending on `er-obs` (which would cycle, and would pull
//! `opentelemetry_sdk` into the compile of every generated query module).
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
//! [`crate::registry`]; the series count grows with every declared statement.
//! Statement identity lives on the span, where it is free and groupable.

use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::metrics::{Counter, Histogram, Meter};
use opentelemetry::{KeyValue, global};

/// The instruments, built once when a meter provider is available.
struct Instruments {
    query_duration: Histogram<f64>,
    spans_ended: Counter<u64>,
    spans_exported: Counter<u64>,
    span_exports_failed: Counter<u64>,
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
        spans_ended: meter
            .u64_counter("er.telemetry.spans_ended")
            .with_description("Spans that ended in this process and entered the export pipeline.")
            .build(),
        spans_exported: meter
            .u64_counter("er.telemetry.spans_exported")
            .with_description("Spans the exporter successfully delivered to the collector.")
            .build(),
        span_exports_failed: meter
            .u64_counter("er.telemetry.span_exports_failed")
            .with_description("Export batches the collector rejected or could not receive.")
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

/// Whether the instruments exist, i.e. a meter provider was installed.
///
/// Read by [`crate::executions`] to decide whether a statement span is worth
/// observing at all when per-run buffering is off. Keeping it a cheap
/// `OnceLock::get` means the no-telemetry case — `er-entity`, `dedupe`, every
/// test binary — still pays nothing.
pub(crate) fn installed() -> bool {
    INSTRUMENTS.get().is_some()
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

/// One span reached the end of the pipeline's in-process half: it ended, the
/// sampler kept it and the SDK handed it to the span processors.
///
/// This is the upstream of the two export counters, and the reference they are
/// read against: `spans_ended` rising while `spans_exported` stays flat is the
/// signature of the wedge #941 observed — a pod whose telemetry looks installed
/// and exports nothing — while `span_exports_failed` rising instead says the
/// collector or the network is the wedge. No dimension value is defaulted: the
/// role is known at every call site, because the exporter is built per role.
pub(crate) fn record_span_ended(role: crate::Role) {
    let Some(instruments) = INSTRUMENTS.get() else {
        return;
    };
    instruments
        .spans_ended
        .add(1, &[KeyValue::new("er.role", role.as_str())]);
}

/// `count` spans left the process in one accepted export batch.
pub(crate) fn record_spans_exported(role: crate::Role, count: u64) {
    let Some(instruments) = INSTRUMENTS.get() else {
        return;
    };
    instruments
        .spans_exported
        .add(count, &[KeyValue::new("er.role", role.as_str())]);
}

/// One export batch failed — the collector refused it or the network did.
/// Counted per batch rather than per span: the operationally meaningful unit
/// is the rejected POST, and the per-span count is recoverable from the batch
/// size at the call site.
pub(crate) fn record_span_export_failure(role: crate::Role) {
    let Some(instruments) = INSTRUMENTS.get() else {
        return;
    };
    instruments
        .span_exports_failed
        .add(1, &[KeyValue::new("er.role", role.as_str())]);
}

#[cfg(test)]
pub(crate) mod test_support {
    //! One in-memory meter for the whole test binary, and the lock that makes
    //! reading it back deterministic.
    //!
    //! `INSTRUMENTS` is a `OnceLock`, so two tests each installing their own
    //! meter would race — the loser asserts against a meter nobody records
    //! into. And `get_finished_metrics` DRAINS the exporter, so two tests
    //! flushing concurrently can steal each other's data points. Both races
    //! are removed here: one shared provider wins the `OnceLock` regardless of
    //! order, and the mutex serialises record → flush → read sequences.
    //!
    //! The drain means a reader sees every series recorded so far, not just
    //! its own — so assertions must be `any(name == …)` over the accumulated
    //! set, never an exact match on the whole export.

    use std::sync::{Mutex, OnceLock};

    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    static TEST_METER: OnceLock<(SdkMeterProvider, InMemoryMetricExporter)> = OnceLock::new();

    pub(crate) fn test_meter() -> &'static (SdkMeterProvider, InMemoryMetricExporter) {
        TEST_METER.get_or_init(|| {
            let exporter = InMemoryMetricExporter::default();
            let provider = SdkMeterProvider::builder()
                .with_reader(PeriodicReader::builder(exporter.clone()).build())
                .build();
            super::install(&provider.meter("test"));
            (provider, exporter)
        })
    }

    pub(crate) fn metric_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_is_safe_whether_or_not_a_provider_exists() {
        // Renamed from `recording_without_a_provider_is_a_no_op`, which
        // overstated what it can guarantee: `INSTRUMENTS` is a process-global
        // `OnceLock`, and `executions.rs`'s behavioural test installs a real
        // meter in the same binary, so by the time this runs the provider may
        // or may not exist depending on test order.
        //
        // "Does not panic" is true either way, and is the property that
        // matters — a metric that cannot be recorded must never fail a
        // statement. Asserting a no-op would have been an order-dependent
        // claim dressed as an invariant.
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

        // The telemetry self-counters all carry exactly the role dimension —
        // the whole point is telling WHICH role went silent.
        for name in [
            "er.telemetry.spans_ended",
            "er.telemetry.spans_exported",
            "er.telemetry.span_exports_failed",
        ] {
            let declared = crate::registry::METRICS
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("{name} is declared in the registry"));
            assert_eq!(declared.dimensions, &["er.role"], "{name}");
            assert_eq!(declared.unit, "1", "{name}");
        }
    }
}
