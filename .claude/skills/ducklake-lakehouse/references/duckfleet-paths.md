# DuckFleet execution-path support envelope — #1170

Decision: **remote execution is supported only for generated reads and the
account golden task inside `run_all` on DuckDB 2.0.1**. With
`ER_DUCKFLEET_TASK_POLICY` installed, every other pod task runs embedded, and
the pod logs the decision and its reason. Nothing falls back silently: the
routing is a fixed, exhaustively tested table, not a fallback.

## Path matrix

| Path | Backend with a remote policy | Evidence |
|---|---|---|
| Generated dataset reads | **Remote** | `make check-duckfleet-reads`: exact float bits, typed nulls, filtered/keyset snapshots, rollback, cancellation and zero leaked sessions. |
| Account golden in `run_all_full` / `run_all_incremental` | **Remote** | `make check-duckfleet-task`: embedded parity, 4 golden records, 16 lineage rows, 3 scores, 10 correlated statements, rerun equality and writer exclusion. Person datasets are refused explicitly. |
| Account golden publication/recovery | **Remote** | Fence, write-ahead journal and lake receipt. A lost COMMIT reply reconciles `committed`; an unsent COMMIT reconciles `rolled_back`. |
| `golden` in `correct` / `apply_corrections` | Embedded | Same body, never qualified remotely. |
| `ingest`, `standardize`, `match`, `reconcile`, `train`, `profile`, `extract`, `write_back_scan`, `apply_staged` | Embedded | No qualified remote adapter. |
| `reconcile_schema`, `lake_init`, `maintain` | Embedded | Lake administration stays with the embedded reconciler (catalog version policy). |
| `init`, `assemble`, `finish` | Embedded | Run bookkeeping, not a remote workload. |
| Control nodes (provisioning, approvals, action drains, model registration, cursors) | Control plane | Touch no lake engine. |

`er_server::duckfleet_tasks::route(kind, task)` is the table, and it has no
default arm. `every_pod_task_has_an_explicit_route` walks `dag_for(kind)` for
every `JobKind`, fails on an unrouted pod task, and pins the remote set to
run_all's golden. An embedded decision under a policy logs `DuckFleet-enrolled
pod runs this task embedded` with `er.task_id`, `job.kind` and `reason`.

Why embedded rather than refused: the policy is set on the one shared task
template (`ER_TASK_TEMPLATE_PATH`), so every task pod of an enrolled job carries
it. #1182 briefly refused every non-golden task. That failed each enrolled
`run_all` at `init` before `golden` could run; `make check-duckfleet-task` on
`e82b4582` showed `init: failed` and every stage `upstream_failed` (#1183).
Embedded tasks keep one writer per dataset: they run under the dataset job slot,
and the remote-attempt fence holds every writer while an attempt is unresolved.

Removing the policy selects embedded execution for new pods. It does not move an
existing remote session or reconcile an uncertain write; the account golden
fence and receipt still govern that recovery.

## Compatibility and rollout boundaries

#1169 qualified the remote rows above against upstream `778dbe02` on DuckDB
2.0.1 with DuckLake 1.0. It did not qualify full-pipeline remote execution,
production rollout, AWS bucket policy, provider economics or every nested type.
#1134 owns customer-scoped API workflow, deployment/retention checks, measured
thresholds, operational ownership and the go/no-go decision.

The generated read and account golden paths retain the existing public API and
embedded default. The routing log line is the support-envelope statement:
operators can see which path each task took and why.
