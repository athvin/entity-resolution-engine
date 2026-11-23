# DuckFleet remote-attempt recovery

Issue #1133 makes a remote golden task's failure safe to recover from. The
invariant is one sentence: **a dataset admits no writer, of any backend, while a
remote attempt on it might still publish.** Nothing is ever replayed to find out
what happened. DuckFleet's `retry_safe` describes one request, not permission to
re-run an ER job, and no automatic job retry is added here (#530 owns retries).

## Contents

- [The fence](#the-fence)
- [The publication receipt](#the-publication-receipt)
- [The failure matrix](#the-failure-matrix)
- [Operating bounds](#operating-bounds)
- [What is proven where](#what-is-proven-where)

## The fence

`remote_attempts` (control-plane Postgres) holds one row per remote attempt; a
partial unique index allows one unresolved row per dataset, and the natural key
allows one attempt per task launch, so a restarted pod never inherits a prior
session. `job/update/claim.sql` dispatches no job for a dataset with an
unresolved row, and no org-level job for its org. Every re-run, resume, clear
or embedded run enters through that claim, so a retry and a backend switch are
both held at the database rather than by convention.

The pod writes the row over the attempt channel,
`POST /v1/internal/attempts/{attempt_id}/remote`, authenticated by the attempt
token. Org, dataset, job and launch are copied from the task row; the body
carries only UUIDs, a namespace name and bounded phases. Writes require the
task to be live, running, on the same launch and the registered pod, so a
fenced or superseded pod is refused.

| Step | Journaled before | Refusal means |
|---|---|---|
| `acquire` | opening any session | another attempt holds the dataset, or the pod is stale |
| `session` | any SQL on the session | no SQL is sent; the session is closed |
| `request` (`commit` flagged) | each request is sent | that request and every later one are not sent |
| `unknown` | — | the owner could not establish the outcome |
| `release` | — | only `committed`, `rolled_back`, `not_executed` |

Journal-before-send is what makes an unjournaled COMMIT proof of non-publication.

## The publication receipt

The golden transaction runs
`CALL lake.set_commit_message('er', 'golden', extra_info => '{"remote_attempt":"<id>"}')`
before COMMIT. DuckLake writes it into the snapshot row the commit creates, so
`ducklake_snapshots('lake').commit_extra_info` contains it exactly when the
transaction committed. Requalified on DuckDB 2.0.1 with DuckLake 1.0 on both
sides: committed → present, rolled back → absent.

## The failure matrix

`er_duckfleet::execution::recovery::decide` is the matrix; its unit tests are
one per row. The owner settles first, from what it saw; the dispatcher sweep
settles the rest once the owning launch is no longer live.

| Evidence | Outcome | Who |
|---|---|---|
| COMMIT returned a statement result | `committed` | owner |
| COMMIT never journaled, some request sent | `rolled_back` | owner, or sweep |
| nothing journaled | `not_executed` | owner, or sweep |
| receipt present | `committed` | sweep (whatever the service says) |
| COMMIT journaled, owner still live | **held** | — |
| receipt or COMMIT record unreadable, binding revoked or replaced | **held** | — |
| COMMIT still `accepted`/`executing` | **held** | — |
| COMMIT `statement_result` but no receipt | **held** (contradiction) | — |
| COMMIT terminal `not_executed`/`unknown`, or never admitted, no receipt, grace elapsed | `rolled_back` | sweep |

A COMMIT that failed with a definite SQL error is still left to the sweep: the
owner releases only on a returned or unsent COMMIT.

Public job errors stay actionable without leaking anything: the golden task's
failure text gains `this dataset is held by remote attempt <id> …` when the
fence was not released. No SQL, bind value, profile or credential is stored or
logged; log lines correlate `er.job_id`, `er.task_instance_id`, `er.dataset`
and `remote_attempt`, and `duckfleet.query` spans carry the request UUID that
the journal records.

## Operating bounds

- **Grace** (`ER_DUCKFLEET_RECOVERY_GRACE_SECONDS`, default 900 s) must exceed
  the longest a COMMIT delivery can still be in flight — a statement deadline of
  up to 600 s plus 135 s of queue and delivery allowance — and the time a dead
  worker's catalog transaction takes to resolve.
- **Sweep** every 30 s in the dispatcher loop; a held row is logged each pass
  with its reason.
- **Cancellation**: a job cancel fences the pod at its next heartbeat (30 s
  slices); the pod's monitor then posts DuckFleet `cancel` within 100 ms and
  closes the session, which DuckFleet acknowledges with `closed: true` before
  releasing the worker. A deadline is an interruption request, not a stop:
  upstream allows a 615 s drain inside a 630 s container grace, and external I/O
  can outlive it. An unacknowledged close is logged and leaves cleanup to
  upstream idle bounds; it does not change what was published.
- **Recovery binding**: `ER_DUCKFLEET_RECOVERY_REGISTRY` and
  `ER_DUCKFLEET_RECOVERY_CREDENTIALS_DIR` (absolute) on the dispatcher. Evidence
  is read only through the attempt's pinned generation; `draining` is allowed
  (that is when attempts are drained), `revoked` or a replacement generation is
  not, because upstream request lookups are owner-scoped and another principal
  would report a real request as absent. Unset, attempts with a journaled
  COMMIT stay held.

## What is proven where

- `er-duckfleet` unit tests: journal ordering and refusal, transport loss during
  temporary work, during lake writes and around COMMIT (sent exactly once),
  cancellation and acknowledged cleanup, strict request-record parsing, and every
  `decide` row.
- `pg_remote_attempt_fence`: scope from the token, stale pods and relaunches
  refused, COMMIT journaled once, every writer held, and the sweep releasing only
  on conclusive evidence.
- `acct_e2e`: the receipt is atomic with publication on a real DuckLake.
- `make check-duckfleet-task`: the real service runs the journaled task,
  receipt and owner release end to end, then two faults through an in-test
  relay. A COMMIT whose reply is lost after it landed fails the job, holds the
  dataset and reconciles `committed` with exactly one receipt. A COMMIT that
  never reaches the gateway reconciles `rolled_back` with the lake unchanged.
  `fault_outcomes` in `duckfleet-task-evidence.json` records the run.
- Not exercised against the real service: gateway restart and worker-process
  loss. Upstream marks such requests terminal with an `unknown` outcome
  (`interrupt_abandoned`); that is the same row of the matrix as the lost
  COMMIT reply, settled by the receipt.

#1169 reruns the complete proof against upstream `778dbe02` on DuckDB 2.0.1.
The lost-reply case reconciles `committed` with one receipt; the unsent case
reconciles `rolled_back` with no receipt. The other fence, journal, owner and
sweep invariants are unchanged.
