//! Bounded operational instruments for dashboards and paging.
//!
//! Callers report committed transitions, never attempted writes. These counters
//! are operational observations, not a billing ledger: a process can die between
//! the database commit and export. Tenant identities remain in spans and tables.

use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};

/// The closed resource-class vocabulary, including malformed/legacy job rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Small,
    Medium,
    Large,
    Other,
}

impl Class {
    pub const ALL: [Self; 4] = [Self::Small, Self::Medium, Self::Large, Self::Other];

    #[must_use]
    pub fn from_value(value: Option<&str>) -> Self {
        match value {
            Some("S") => Self::Small,
            Some("M") => Self::Medium,
            Some("L") => Self::Large,
            _ => Self::Other,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Small => "S",
            Self::Medium => "M",
            Self::Large => "L",
            Self::Other => "other",
        }
    }

    fn attributes(self) -> [KeyValue; 1] {
        [KeyValue::new("resource_class", self.as_str())]
    }
}

struct Instruments {
    http_requests: Counter<u64>,
    http_duration: Histogram<f64>,
    queue_depth: Gauge<u64>,
    queue_total: Gauge<u64>,
    active_tasks: Gauge<u64>,
    snapshot: Counter<u64>,
    dispatch_duration: Histogram<f64>,
    task_duration: Histogram<f64>,
    task_completed: Counter<u64>,
    task_retries: Counter<u64>,
    pod_failures: Counter<u64>,
}

static INSTRUMENTS: OnceLock<Instruments> = OnceLock::new();

pub(crate) fn install(meter: &Meter) {
    let _ = INSTRUMENTS.set(Instruments {
        http_requests: meter.u64_counter("er.http.requests").with_unit("1").build(),
        http_duration: meter
            .f64_histogram("er.http.duration")
            .with_unit("ms")
            .build(),
        queue_total: meter
            .u64_gauge("er.jobs.queue.total")
            .with_unit("1")
            .build(),
        queue_depth: meter
            .u64_gauge("er.jobs.queue.depth")
            .with_unit("1")
            .build(),
        active_tasks: meter.u64_gauge("er.tasks.active").with_unit("1").build(),
        snapshot: meter.u64_counter("er.ops.snapshots").with_unit("1").build(),
        dispatch_duration: meter
            .f64_histogram("er.jobs.dispatch.duration")
            .with_unit("ms")
            .build(),
        task_duration: meter
            .f64_histogram("er.tasks.duration")
            .with_unit("ms")
            .build(),
        task_completed: meter
            .u64_counter("er.tasks.completed")
            .with_unit("1")
            .build(),
        task_retries: meter.u64_counter("er.tasks.retries").with_unit("1").build(),
        pod_failures: meter.u64_counter("er.pods.failures").with_unit("1").build(),
    });
}

/// One non-probe HTTP response; status classes keep client input off dimensions.
pub fn http_response(status: u16, elapsed: Duration) {
    let Some(i) = INSTRUMENTS.get() else { return };
    let status_class = match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    };
    let attrs = [KeyValue::new("status_class", status_class)];
    i.http_requests.add(1, &attrs);
    i.http_duration
        .record(elapsed.as_secs_f64() * 1000.0, &attrs);
}

/// A complete database snapshot. Call once for every class, including zeros.
pub fn queue_snapshot(class: Class, depth: u64, active: u64) {
    let Some(i) = INSTRUMENTS.get() else { return };
    i.queue_depth.record(depth, &class.attributes());
    i.active_tasks.record(active, &class.attributes());
}

/// The queue/concurrency read succeeded; missing samples must not look idle.
pub fn snapshot_succeeded(total_depth: u64) {
    if let Some(i) = INSTRUMENTS.get() {
        i.queue_total.record(total_depth, &[]);
        i.snapshot.add(1, &[]);
    }
}

/// Elapsed time from enqueue to a successful job claim, including backoff.
pub fn job_dispatched(class: Class, elapsed: Duration) {
    if let Some(i) = INSTRUMENTS.get() {
        i.dispatch_duration
            .record(elapsed.as_secs_f64() * 1000.0, &class.attributes());
    }
}

/// A terminal execution outcome, counted only after its guarded write commits.
#[derive(Clone, Copy, Debug)]
pub enum Outcome {
    Done,
    NoOp,
    Failed,
    Skipped,
}

impl Outcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::NoOp => "no_op",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// A task execution ended. Stage labels come from this closed vocabulary, never
/// from arbitrary tenant configuration. Unknown compiled tasks share `other`.
pub fn task_completed(class: Class, task: &str, outcome: Outcome, elapsed: Option<Duration>) {
    let Some(i) = INSTRUMENTS.get() else { return };
    let stage = match task {
        "init" => "init",
        "ingest" => "ingest",
        "standardize" => "standardize",
        "match" => "match",
        "reconcile" => "reconcile",
        "assemble" => "assemble",
        "finish" => "finish",
        "apply_staged" => "apply_staged",
        "write_back_scan" => "write_back_scan",
        "reconcile_schema" => "reconcile_schema",
        "profile" => "profile",
        "maintain" => "maintain",
        "extract" => "extract",
        "train" => "train",
        "lake_init" => "lake_init",
        _ => "other",
    };
    i.task_completed.add(
        1,
        &[
            KeyValue::new("resource_class", class.as_str()),
            KeyValue::new("outcome", outcome.as_str()),
        ],
    );
    if let Some(elapsed) = elapsed {
        i.task_duration.record(
            elapsed.as_secs_f64() * 1000.0,
            &[
                KeyValue::new("resource_class", class.as_str()),
                KeyValue::new("stage", stage),
            ],
        );
    }
}

/// A failed task created a committed successor try.
pub fn task_retried(class: Class) {
    if let Some(i) = INSTRUMENTS.get() {
        i.task_retries.add(1, &class.attributes());
    }
}

/// A confirmed OOM kill or eviction; other free-text reasons never form series.
pub fn pod_failed(class: Class, reason: Option<&str>) {
    let reason = match reason {
        Some("OOMKilled") => "oom",
        Some("Evicted" | "EvictionByEvictionAPI") => "evicted",
        _ => return,
    };
    if let Some(i) = INSTRUMENTS.get() {
        i.pod_failures.add(
            1,
            &[
                KeyValue::new("resource_class", class.as_str()),
                KeyValue::new("reason", reason),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};

    #[test]
    fn exports_bounded_observations_and_clears_an_empty_queue() {
        let _serial = crate::metrics::test_support::metric_test_lock()
            .lock()
            .unwrap();
        let (provider, exporter) = crate::metrics::test_support::test_meter();
        http_response(503, Duration::from_millis(25));
        let unknown = Class::from_value(Some("customer@example.invalid"));
        assert_eq!(unknown, Class::Other);
        queue_snapshot(unknown, 12, 3);
        queue_snapshot(unknown, 0, 0);
        snapshot_succeeded(0);
        job_dispatched(unknown, Duration::from_secs(2));
        task_completed(
            unknown,
            "customer@example.invalid",
            Outcome::NoOp,
            Some(Duration::from_secs(3)),
        );
        task_retried(unknown);
        pod_failed(unknown, Some("OOMKilled"));
        pod_failed(unknown, Some("EvictionByEvictionAPI"));
        pod_failed(unknown, Some("customer@example.invalid"));
        provider.force_flush().unwrap();
        let collected = exporter.get_finished_metrics().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for resource in &collected {
            for scope in resource.scope_metrics() {
                for metric in scope.metrics() {
                    if !metric.name().starts_with("er.") {
                        continue;
                    }
                    let definition = crate::registry::METRICS
                        .iter()
                        .find(|m| m.name == metric.name())
                        .unwrap();
                    let check = |attrs: Vec<&KeyValue>| {
                        let actual: std::collections::BTreeSet<_> =
                            attrs.iter().map(|a| a.key.as_str()).collect();
                        let expected: std::collections::BTreeSet<_> =
                            definition.dimensions.iter().copied().collect();
                        assert_eq!(actual, expected, "{}", metric.name());
                        assert!(attrs.iter().all(|a| !a.value.to_string().contains('@')));
                    };
                    match metric.data() {
                        AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                            for point in sum.data_points() {
                                check(point.attributes().collect());
                                if metric.name() == "er.pods.failures" {
                                    assert_eq!(point.value(), 1);
                                }
                            }
                        }
                        AggregatedMetrics::U64(MetricData::Gauge(gauge)) => {
                            for point in gauge.data_points() {
                                check(point.attributes().collect());
                                assert_eq!(
                                    point.value(),
                                    0,
                                    "an empty queue must replace old values"
                                );
                            }
                        }
                        AggregatedMetrics::F64(MetricData::Histogram(histogram)) => {
                            for point in histogram.data_points() {
                                check(point.attributes().collect());
                            }
                        }
                        _ => continue,
                    }
                    seen.insert(metric.name().to_string());
                }
            }
        }
        for name in [
            "er.http.requests",
            "er.http.duration",
            "er.jobs.queue.depth",
            "er.jobs.queue.total",
            "er.tasks.active",
            "er.ops.snapshots",
            "er.jobs.dispatch.duration",
            "er.tasks.duration",
            "er.tasks.completed",
            "er.tasks.retries",
            "er.pods.failures",
        ] {
            assert!(seen.contains(name), "{name} never reached the exporter");
        }
    }
}
