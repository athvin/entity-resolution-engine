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

Upstream dependency: DuckFleet issue #124 / PR #125 extends validation beyond
60 seconds and adjusts the gateway's bounded wait. Defaults remain unchanged.
A deadline requests interruption, not proof that a commit did not happen.
External I/O can delay when DuckDB observes interruption; a short HTTP wait can
return uncertainty before durable request state records a timeout. Never replay
that SQL to discover what happened. Upstream's bounded shutdown drain can also
interrupt a long statement.

Every remote request carries job, task and attempt IDs in its diagnostic context
and has its own request UUID. `duckfleet.query` spans carry those IDs without SQL,
bind values or credentials. Lost ownership or SIGTERM requests cancellation;
publication rechecks the fence. Execution failures are terminal task failures,
not automatic transport or task retries. Full uncertain-commit, process-loss and
cleanup qualification belongs to #1133; this pilot is not broad production use.

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
#1134 gates. This evidence does not supersede #1128's no-go for interchangeable
whole-pipeline execution across the differing engine builds.
