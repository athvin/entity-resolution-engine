# Tier: is this work in the right place?

The reviewer's question is never "is this slow". It is **which of three
execution tiers the change landed in, and whether the boundary it crossed was
the one the code actually needs.**

## Contents

- [The three tiers](#the-three-tiers)
- [The deciding question is the writer lock, not latency](#the-deciding-question-is-the-writer-lock-not-latency)
- [Findings worth raising](#findings-worth-raising)
- [What a pod costs, so the finding can be priced](#what-a-pod-costs-so-the-finding-can-be-priced)
- [Already structural — do not raise](#already-structural--do-not-raise)
- [Design intent that is not the built shape](#design-intent-that-is-not-the-built-shape)

## The three tiers

| Tier | Runs in | Talks to | Shape | Holds the org slot |
|---|---|---|---|---|
| **Request-synchronous** | the `er-server` request | Postgres through `er_entities::`, or the lake **read-only** with a pinned snapshot | async | no |
| **Control task** | the dispatcher process | Postgres only, on the control plane's own credentials | async | no — and never counts toward `executor.inFlightCap` |
| **Pod task** | a per-attempt Kubernetes Job | the lake, through a `dedupe_core::Session` | synchronous | yes — at most one live pod per tenant |

The authority is `docs/batch-orchestration-design.md` §3.5 and §3.6. The
one-line rule it states, and the one a reviewer can apply from a diff alone:

> **A body that needs a lake open is a pod task.**

`provision` is the worked example of that rule being paid for rather than
argued with: it was split into `create_catalog` (control, creates the database
or the directory) → `lake_init` (pod, because `reconcile_lake_env` opens the
lake) → `activate` (control, one state transition). Three rows instead of one,
because the middle one opens a lake.

The read API is **not** a counter-example. `er-server/src/read.rs` attaches
DuckLake read-only and pins a snapshot, so it holds no writer lock and cannot
be the one-writer-per-tenant conflict. Read-only plus a pinned snapshot is the
property that makes a lake touch request-legal; a diff that drops either is a
tier change wearing the clothes of a read.

## The deciding question is the writer lock, not latency

This is the part worth getting right, because the intuitive rule — "slow work
goes to a pod" — picks the correct tier for the wrong reason and then gets the
next case backwards.

A lake **write** cannot happen in a request at any speed. §4.6 enforces one
writer per tenant with two partial unique indexes (`jobs_one_active_per_org`,
`task_instances_one_pod_per_org`), deliberately in Postgres rather than in
code, because "the queue's correctness is Postgres's job". A request that
opened a lake for writing would be a second writer, and the failure is not
slowness — it is a constraint violation, or a corrupted interleave, on a tenant
whose run happened to be live.

That is why the steward surface is shaped the way it is. A steward action is
written to `staged_steward_actions` **in the request** — a typed Postgres
insert, sub-second — and the lake write happens later, in `apply_corrections`'s
pod tasks, with `drain_actions` and `settle_actions` as the control halves
either side. The user gets an honest "queued, applies after the current job"
instead of a lock wait.

So the reviewer's decision procedure is:

```
Does the change write the lake?            -> pod task. Not negotiable.
Does it open a lake at all?                -> pod task, unless read-only AND
                                              snapshot-pinned, which makes it
                                              request-legal.
Is it Postgres-only, and part of a job?    -> control task. Cheaper than a pod
                                              by the whole §8 overhead.
Is it Postgres-only, and answers a caller? -> request-synchronous. A job row
                                              for this is the finding.
```

## Findings worth raising

Each of these compiles, passes every test, and is wrong.

- **A pod for a request.** A new `JobKind`, or a new task on an existing DAG,
  whose body touches only Postgres. It pays §8's pod overhead and a tenant's
  one-pod slot to run milliseconds of SQL — the design names this explicitly as
  the reason control tasks exist ("3–30 s per step for milliseconds of SQL").
  The fix is a control task, or no task at all.
- **A request doing stage-scale work.** A handler that iterates a relation, or
  calls into `dedupe_core` beyond session setup, or loops over records to
  compute something a declared statement could aggregate. `backend-design.md`
  states the invariant the API's responsiveness rests on — it **never runs
  stages**, and imports `er` only for config validation, the read-API module
  and sub-second steward primitives. A new import that is none of those three
  is the finding, and the import line is the evidence.
- **An unbounded read where the page was the contract.** The read API's
  envelope is `{ items, snapshot, next_cursor }` with a keyset cursor and a
  capped limit, precisely so a request cannot be made to materialize a lake.
  A new reader that returns a bare list, or takes a caller's limit without a
  cap, has removed the bound rather than chosen a different one.
- **A lake write smuggled into a control body.** Structurally gated (see
  below), so you will not find it as a lake call — find it as a *plan* that
  cannot be honoured: a control task whose output is described as written to
  the lake, with the actual write missing. `entity.write_back` is the known
  live example and it is an open item in §12, not a bug to re-report.
- **Work split across the boundary for no reason.** Two contiguous same-class
  pod tasks where one would do. Fusion (§3.7) already groups contiguous
  same-class tasks into one pod, so this is usually a non-finding — raise it
  only when the new task's class *differs*, which is what forces the extra pod.

## What a pod costs, so the finding can be priced

A tier finding needs an order of magnitude, and §8 has measured ones: a task on
a warm node costs roughly three seconds of fixed overhead, and the first task of
a cold run adds about thirty. Quote the section rather than restating its
numbers — they were measured on a dated artifact under
`docs/measurements/` and will be re-measured.

The consequence that stops most "this should be a pod" findings: because
contiguous same-class tasks fuse into one pod, **a chain of stages does not pay
a cold node per link.** An S or M run is one segment and one pod. So "you added
a stage, that is another pod" is usually false.

## Already structural — do not raise

- A control body that links `er_cli::`, `dedupe_core::` or `open_lake` —
  `er-server/tests/control_tasks_no_engine.rs` scans every file under
  `src/dags/control/` and fails.
- A `JobKind` the queue can hold with nowhere to go —
  `tests/dispatch_coverage.rs` checks both destinations.
- A second executor growing in the product — `tests/no_second_executor.rs`.
- A DAG with two unordered lake-writing tasks — refused at assembly (§3.3), and
  `verify_all` runs every assembly at boot, so it is a refusal to start.

## Design intent that is not the built shape

`backend-design.md`'s risk register describes steward writes trying **inline
with a short lock timeout** and falling back to staging on conflict. That is not
what shipped: the built path stages unconditionally. Do not cite the inline
variant as the rule a diff violates, and do not report the absence of the lock
timeout as a gap — it is an unbuilt option, and `rg lock_timeout rust/crates`
is the one-command check.
