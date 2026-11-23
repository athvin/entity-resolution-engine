# DuckFleet execution-path support envelope — #1170

Decision: **remote execution is supported only for generated reads and the
account golden task on DuckDB 2.0.1**. Every other dispatcher/API execution path
uses embedded execution by default and is explicitly refused when an operator
installs `ER_DUCKFLEET_TASK_POLICY`. Nothing silently falls back to embedded
execution while that policy is present.

## Path matrix

| Path | Backend on 2.0.1 | Evidence |
|---|---|---|
| Generated dataset reads | **Remote** | `make check-duckfleet-reads`: exact float bits, typed nulls, filtered/keyset snapshots, rollback, cancellation and zero leaked sessions. |
| Account golden generation | **Remote** | `make check-duckfleet-task`: embedded parity, 4 golden records, 16 lineage rows, 3 scores, 10 correlated statements, rerun equality and writer exclusion. |
| Account golden publication/recovery | **Remote** | Fence, write-ahead journal and lake receipt. Lost COMMIT reply reconciles `committed`; unsent COMMIT reconciles `rolled_back`. |
| Ingest | **Refused with remote policy** | `selection_is_operator_only_and_scoped_to_golden` proves the task fails before lake opening. |
| Standardize | **Refused with remote policy** | Same regression. |
| Match | **Refused with remote policy** | Same regression. |
| Reconcile | **Refused with remote policy** | Same regression. |
| Explicit training (`train`) | **Refused with remote policy** | Same regression. |
| Correction pass (`correct`, `apply_corrections`) | **Refused with remote policy** | Same regression. |
| Approval application (`apply_approved`) | **Refused with remote policy** | Same regression. |
| Maintenance (`lake_maintain`, `profile`) | **Refused with remote policy** | Same regression. |
| Schema migration (`lake_migrate`) | **Refused with remote policy** | Same regression. |
| Org/dataset provisioning (`provision`, `dataset_provision`) | **Refused with remote policy** | Same regression. |
| Extraction (`extract`) | **Refused with remote policy** | Same regression. |

`duckfleet_tasks::selected` is the single gate before any embedded lake opens.
It accepts a policy only for `golden`; every other task returns
`DuckFleet remote execution is not qualified for task <name>; remove
ER_DUCKFLEET_TASK_POLICY to use the supported embedded backend`. The unit test
enumerates every task kind and asserts both the refusal and the supported
recovery action. Because the error is raised before `open_order_session`, a
refused path cannot execute SQL and cannot be mistaken for a skip.

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
embedded default. Refusal is a support-envelope statement, not an error to hide:
operators and customers can see exactly which path was requested and how to run
it safely.
