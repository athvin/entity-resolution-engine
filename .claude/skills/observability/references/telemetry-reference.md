# Telemetry reference

GENERATED from `rust/crates/er-obs/src/registry.rs` by its own test. Do not
hand-edit: `cargo test -p er-obs reference` byte-compares this file, and
`UPDATE_EXPECT=1 cargo test -p er-obs reference` rewrites it.

A metrics table written by hand is how a metric ends up documented and never
emitted, which reads as coverage rather than as a gap — its dashboard panel
renders an honest-looking empty chart. The registry is therefore the source
of truth and this file is its output.

## Metrics

| Metric | Kind | Unit | Dimensions | Series |
|---|---|---|---|---|
| `er.db.query.duration` | histogram | `ms` | `db.system.name`, `outcome` | 4 |
| `er.telemetry.spans_ended` | counter | `1` | `er.role` | 4 |
| `er.telemetry.spans_exported` | counter | `1` | `er.role` | 4 |
| `er.telemetry.span_exports_failed` | counter | `1` | `er.role` | 4 |

**16 series in total.**

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

