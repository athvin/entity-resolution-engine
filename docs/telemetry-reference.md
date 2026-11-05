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

**4 series in total.**

### `er.db.query.duration`

How long a SQL statement took, aggregated across every statement. Statement identity is deliberately NOT a dimension — see the module docs for the arithmetic — so this answers 'is the database slow' and not 'which statement is slow'. The second question is a span query, where `er.statement` is recorded and costs nothing.

## Dimensions a metric may never carry

Each is unbounded or tenant-scoped, and each would turn one metric into
hundreds of billable series that grow without anyone choosing to grow them.
Under `awsemf` a distinct dimension set is a CloudWatch custom metric at
roughly $0.30/month: `statement_name` alone is 247 statements x 2 outcomes =
494 series, about $148/month, for one histogram in one developer's dev
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

