# DuckFleet account golden-task pilot

Issue #1132 selects **account golden-record generation** as the first remote
write task. The DAG now runs `golden` between `reconcile` and `assemble`;
`assemble` retains reviews, metrics and quality. Person golden generation remains
embedded and retains both override passes. Remote selection refuses person
configuration explicitly. This is not remote training or remote matching.

Both account backends call `er_cli::golden_task::run_account_guarded`, using the
existing account survivorship generators. One pinned session builds native
`TEMP` lineage and records, then replaces both lake outputs in one transaction.
An attempt-fence check precedes publication. The existing dataset job slot still
orders writers; the remote branch opens no embedded lake alongside its writer.
The normal work-order, heartbeat, finish and job-settlement channel is retained.

## Contents

- [Operator configuration](#operator-configuration)
- [Deadlines and diagnostics](#deadlines-and-diagnostics)
- [Qualification and limits](#qualification-and-limits)
- [DuckDB 2.0.1 requalification](#duckdb-201-requalification)

## Operator configuration

Task pods use embedded execution unless their operator-owned template sets
`ER_DUCKFLEET_TASK_POLICY` to an absolute JSON file path. Only the `golden` task
consults it. Mount the policy, registry and projected credentials read-only into
the task pod; these paths and compute choices never come from API job parameters.
No environment enables the pilot by default.

A policy has this shape (capacity is an example, not an enrollment instruction):

```json
{
  "registry": "/etc/er-duckfleet/registry.json",
  "credentials": "/etc/er-duckfleet/credentials",
  "statement_seconds": 120,
  "classes": {
    "M": {"mode": "isolated", "cpu": 6, "memory_gib": 12, "disk_gib": 150}
  }
}
```

The registry and credential layout is the same as the generated-read pilot.
Every attempt reloads and resolves the active org/dataset binding. The session
retains that admission snapshot. A missing/revoked binding, absent class, invalid
policy or unavailable placement fails the task; none falls back to embedded.
Removing the policy environment variable selects embedded for newly launched
pods. It does not move an existing remote session or reconcile an uncertain write.

`persistent` means DuckFleet's default one-CPU, 1-GiB-memory, 2-GiB-disk worker.
Explicit isolated sizing is checked against the task's `ER_DUCKDB_THREADS` and
`ER_DUCKDB_MEMORY_LIMIT`; DuckFleet reserves half its container memory for DuckDB.
The returned worker placement must exactly match the request. An undersized
persistent worker is refused, even if it could execute the tiny qualification
fixture. Production class M therefore cannot silently become the default worker.

## Deadlines and diagnostics

Statements accept 1–600 seconds, with the policy selecting the bound. The task
transport allows the statement plus the maximum queue allowance and delivery
margin; session opening remains bounded separately. The read pilot retains its
existing 60-second statement and 75-second transport limits.

Upstream dependency: [DuckFleet PR #125](https://github.com/athvin/duck-fleet/pull/125) extends validation beyond
60 seconds and adjusts the gateway's bounded wait. Defaults remain unchanged.
A deadline requests interruption, not proof that a commit did not happen.
External I/O can delay when DuckDB observes interruption; a short HTTP wait can
return uncertainty before durable request state records a timeout. Never replay
that SQL to discover what happened. Upstream allows a 615-second drain inside a
630-second container grace; idle drains remain short. External I/O can still outlive an interruption request.

Every remote request carries job, task and attempt IDs in its diagnostic context
and has its own request UUID. `duckfleet.query` spans carry those IDs without SQL,
bind values or credentials. Lost ownership or SIGTERM requests cancellation;
publication rechecks the fence. Execution failures are terminal task failures,
not automatic transport or task retries. An uncertain outcome holds the dataset
until it is reconciled from evidence, as the skill's recovery reference describes.
This pilot is not broad production use.

## Qualification and limits

`scripts/duckfleet/task.py` provisions fresh Compose/Postgres/object-store volumes
and drives the public job endpoint through real `exec-task` child processes.
Kubernetes itself is the existing child-process API double. The fixture shares
one disposable catalog between the embedded stages and the remote golden task.
It compares account golden records and lineage, preserves matching evidence,
checks remote history correlation, task reruns, queued-writer exclusion and a
terminal unavailable-capacity outcome. `assembled_at` is intentionally volatile;
rollback preserves it, while successful-rerun comparisons exclude it.

The fixture's one-thread, 256-MB engine budget fits the persistent worker.
Production placement, AWS identity and cross-dataset qualification are separate
#1134 gates. Whole-pipeline remote execution is not claimed by this pilot.

Run `make check-duckfleet-task` to build the pinned upstream revision and repeat
that proof. Recorded `duckfleet-task-evidence.json` evidence identifies the
source and actual worker image. It records 4 golden records, 16 lineage rows,
3 matching scores and 10 correlated remote statements (the tenth is the publication receipt).

A separate controlled external-I/O statement (`deadline` in the evidence,
re-measured at `778dbe02` on DuckDB 2.0.1) completed in 65.221 seconds under a
120-second deadline; 601 seconds was refused. Under a one-second deadline the
same I/O settled `timed_out` after 65.175 seconds, so external I/O observes
cancellation late. Reproduce it with upstream `scripts/test-long-deadline.py
--image er-duckfleet-task-<rev12>:proof` (the image `make check-duckfleet-task`
builds), using Python with PyArrow installed.

## DuckDB 2.0.1 requalification

#1169 reruns the complete task proof against upstream `778dbe02` with both
sides on DuckDB 2.0.1 and DuckLake 1.0. Embedded/remote results and the rerun
match, writer exclusion and undersized-capacity refusal hold, and both COMMIT
faults reconcile as documented. The evidence is a qualified account golden-task
envelope, not proof that every other task is supported remotely.
