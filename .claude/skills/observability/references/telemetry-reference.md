# Telemetry reference

GENERATED from `rust/crates/er-obs/src/registry.rs` by its own test. Do not
hand-edit: `cargo test -p er-obs reference` byte-compares this file, and
`UPDATE_EXPECT=1 cargo test -p er-obs reference` rewrites it.

A metrics table written by hand is how a metric ends up documented and never
emitted, which reads as coverage rather than as a gap — its dashboard panel
renders an honest-looking empty chart. The registry is therefore the source
of truth and this file is its output.

## Contents

- [Metrics](#metrics)
- [Dimensions a metric may never carry](#dimensions-a-metric-may-never-carry)

## Metrics

| Metric | Kind | Unit | Dimensions | Series |
|---|---|---|---|---|
| `er.jobs.queue.oldest_age` | gauge | `s` |  | 1 |
| `er.pods.pending.oldest_age` | gauge | `s` |  | 1 |
| `er.pods.snapshots` | counter | `1` |  | 1 |
| `er.jobs.queue.total` | gauge | `1` |  | 1 |
| `er.http.requests` | counter | `1` | `status_class` | 6 |
| `er.http.duration` | histogram | `ms` | `status_class` | 6 |
| `er.jobs.queue.depth` | gauge | `1` | `resource_class` | 4 |
| `er.tasks.active` | gauge | `1` | `resource_class` | 4 |
| `er.ops.snapshots` | counter | `1` |  | 1 |
| `er.jobs.dispatch.duration` | histogram | `ms` | `resource_class` | 4 |
| `er.tasks.duration` | histogram | `ms` | `resource_class`, `stage` | 64 |
| `er.tasks.completed` | counter | `1` | `resource_class`, `outcome` | 16 |
| `er.tasks.retries` | counter | `1` | `resource_class` | 4 |
| `er.pods.failures` | counter | `1` | `resource_class`, `reason` | 8 |
| `er.db.query.duration` | histogram | `ms` | `db.system.name`, `outcome` | 4 |
| `er.telemetry.spans_ended` | counter | `1` | `er.role` | 4 |
| `er.telemetry.spans_exported` | counter | `1` | `er.role` | 4 |
| `er.telemetry.span_exports_failed` | counter | `1` | `er.role` | 4 |

**137 series in total.**

### `er.jobs.queue.oldest_age`

Age since creation of the oldest currently queued job, including backoff. Healthy empty snapshots emit zero; require er.ops.snapshots before treating the gauge as current.

### `er.pods.pending.oldest_age`

Maximum age since Kubernetes creation among current managed Pending pods. Healthy empty snapshots emit zero; require er.pods.snapshots before treating the gauge as current. Launch intentions without a pod do not count.

### `er.pods.snapshots`

Successful complete managed-pod observations with usable Pending-pod creation timestamps. Zero or absent samples mean the Kubernetes read, dispatcher or export path needs attention; independent of the database snapshot heartbeat.

### `er.jobs.queue.total`

Total queued jobs in the same snapshot as the per-class gauges. The backlog alarm uses Minimum over each period so separate class peaks cannot manufacture a sustained backlog.

### `er.http.requests`

Non-probe API responses by status class; 5xx counts server errors.

### `er.http.duration`

Time to an API response, excluding health probes. Histograms include every observed response before trace sampling.

### `er.jobs.queue.depth`

Current queued jobs including delayed jobs. Zero is emitted for empty classes; pair with er.ops.snapshots to distinguish silence from idle.

### `er.tasks.active`

Live pod-task claims, including queued, running, restarting and canceling. These are claims, not a count of ready Kubernetes pods.

### `er.ops.snapshots`

Successful complete queue/concurrency snapshots. Missing samples mean that the dispatcher, database read or export path needs attention.

### `er.jobs.dispatch.duration`

Elapsed enqueue-to-claim time including backoff, recorded after a successful claim.

### `er.tasks.duration`

Committed pod-task execution durations by bounded stage and class, from durable start/finish timestamps. Includes failed executions; excludes tasks without a start timestamp.

### `er.tasks.completed`

Observed committed pod-task outcomes: done, no_op, failed or skipped. A stale terminal report does not increment it. Operational observations can be lost on process death; use the task ledger for accounting.

### `er.tasks.retries`

Committed successor tries after execution failures. Infrastructure relaunches within a try and operator clears are not retries.

### `er.pods.failures`

Confirmed OOM kills and evictions observed through guarded task transitions. Free-text terminal reasons cannot become dimensions.

### `er.db.query.duration`

How long a SQL statement took, aggregated across every statement. Statement identity is deliberately NOT a dimension — see the module docs for the arithmetic — so this answers 'is the database slow' and not 'which statement is slow'. The second question is a span query, where `er.statement` is recorded and costs nothing.

### `er.telemetry.spans_ended`

Spans that ended in this process and entered the SDK's export pipeline. The upstream reference for the two export counters: this rising while `er.telemetry.spans_exported` stays flat is the wedge signature of #941 — a process whose span pipeline looks installed and exports nothing, with no error logged anywhere. The absent-spans alarm in `envs/dev/alarms-telemetry.tf` fires on exactly that divergence.

### `er.telemetry.spans_exported`

Spans the exporter delivered to the collector, counted as batches are accepted. The counter that flatlines when span export dies — which it did for one whole er-api pod lifetime on 2026-11-08 while every other signal from the same process kept flowing.

### `er.telemetry.span_exports_failed`

Export batches that failed — the collector refused the POST or the network did. Counted per batch (one rejected POST), not per span: the operationally meaningful unit is the failed delivery, and a failure also logs a warning at the call site, so the counter is the alarm input and the log line is the detail.

## Dimensions a metric may never carry

Each is unbounded or tenant-scoped, and each would turn one metric into
hundreds of billable series that grow without anyone choosing to grow them.
Under `awsemf` a distinct dimension set is a CloudWatch custom metric at
roughly $0.30/month: pairing each declared statement with two outcomes
creates hundreds of series for one histogram in one developer's dev
namespace.

All of them are available on spans, which is where high-cardinality identity
belongs. `tests/cardinality.rs` enforces this.

- `statement`
- `statement_name`
- `er.statement`
- `org`
- `er.org`
- `org_id`
- `dataset`
- `er.dataset`
- `dataset_id`
- `run_id`
- `er.run_id`
- `job_id`
- `er.job_id`
- `task_instance_id`
- `record_key`
- `er.record_key`
- `entity_id`
- `er.entity_id`
- `er.action`
- `url.path`
- `url.query`

