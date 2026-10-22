---
name: ducklake-performance
description: The measurement discipline and storage-tuning rules for DuckDB/DuckLake performance work in this repo — how to read span and resource evidence without over-claiming, what the committed historical measurements do and do not license you to conclude, which layout levers exist and the experiment matrix a layout proposal must cover, and the live Rust benchmark surface. Use when a stage or query is slow, when proposing a partition, sort, index or extension change, when reading a profiling artifact or a measurement record, or when comparing a candidate against a baseline. For attach options, maintenance ordering and the no-indexes rule see ducklake-lakehouse; for matching internals see er-matching.
---

# DuckLake performance

Two rules govern everything here:

1. **Evidence before layout.** Changing a partition key or a sort order on the
   strength of an operator name is a guess. Measure, then propose one variable.
2. **Full reloads and incremental runs are separate primary workloads.** Report
   both. A regression in either is a regression — a change that halves reload
   time and slows incremental writes is not an improvement.

## What tooling actually exists today

Read this before planning any measurement, because the answer changed:

| Surface | State |
|---|---|
| The Python/Splink campaign harness and its Compose-based driver | **retired** — removed from the tree |
| `docs/performance.md`, `docs/profiling.md` | **historical records**, both banner-marked as such. Preserved for their measurements and findings |
| `docs/measurements/*.json` | the committed measurement records from those campaigns |
| `cargo bench -p dedupe-core` (`benches/pipeline.rs`) | the live benchmark surface |
| the 50k scale goldens | gated, and run on main/nightly rather than per-PR |

So there is **no campaign driver to run**. Anything that reads like "regenerate
the report with the campaign script" refers to tooling that no longer exists.
A new measurement means the Rust bench surface, or a controlled manual
comparison you design yourself.

The historical record is still valuable — it is where the findings about thread
scaling and the rejected experiments live — but it measured a different
implementation. Treat it as prior art, not as a current baseline. Specifically:
do **not** compare a number you measured today against a figure from those
campaigns and call it a delta.

## Reading evidence without over-claiming

These rules are implementation-independent, and they are where most analysis
goes wrong:

- **Never sum nested durations.** Parent and child spans overlap; adding them
  double-counts. Use the stage ledger and monotonic timestamps.
- **Separate command startup from stage wall time.** Startup is real cost but it
  is not the stage's cost.
- **CPU time can exceed wall time.** That is parallel execution, not a
  measurement error — and it means CPU totals cannot be read as elapsed time.
- **Use unprofiled control time as the end-to-end result.** Profiling has
  overhead; report it separately. Never compare a profiled time against a
  prior unprofiled one.
- **Peaks are high-water marks**, not per-query allocations, and not additive.
  cgroup memory includes page cache; process RSS can double-count shared pages.
- **Missing evidence is not zero.** An unavailable catalog timing cannot be
  inferred from overall latency, and an unprofiled thread is not an idle one.
- **A local object-store stand-in cannot establish remote latency or
  bandwidth.** It can establish CPU and plan behaviour.

Setup, data generation, validation and full-rescore references sit **outside**
workload timing. Decide that boundary before measuring, not after seeing the
numbers.

Full checklist, including the comparability keys that decide whether two runs
are even comparable:
[references/measurement-discipline.md](references/measurement-discipline.md).

## Where to look when something is slow

By workload:

- **Full reload** — rank the stages: standardization, training (per estimation
  method and per blocking rule), scoring, reconciliation, golden/lineage
  assembly. Keep *training pairs* and *prediction candidates* distinct; they are
  different populations and a sample cap on one is a quality decision, not just
  a speed one.
- **Incremental** — look at preparation, new-versus-corpus and new-versus-new
  matching separately, affected-record expansion, cluster updates, and
  touched-entity assembly. A small input batch can trigger much larger reads or
  rewrites, which is the usual surprise.
- **An expensive query** — actual versus estimated cardinalities, rows scanned
  versus returned, filter and projection pushdown, join fan-out, repeated
  scans, sorts and windows, file reads, spill. Link every claim to a specific
  query or operator.

For the matching stages specifically, blocking is the lever that decides the
entire candidate population — see the er-matching skill's cost-preview note
before tuning anything downstream of it.

## Layout: the levers and the experiment matrix

DuckLake has **no secondary indexes** (`CREATE INDEX` is rejected), so the
levers are partitioning and sort-on-write. Do **not** propose an ART index on a
DuckLake table; that is a local-DuckDB-scratch option only.

A layout proposal must test **four** arms, not two:

```
baseline │ ordered writes │ partitioning │ partitioning + within-partition ordering
```

and must account for migration cost, small-file growth, and compaction —
because a layout that reads faster can write slower, and incremental files
weaken clustering over time.

Storage boundaries, the pruning mechanisms that actually apply, and the full
rules: [references/tuning.md](references/tuning.md).

## What a recommendation must carry

One variable per experiment, ordered by likely end-to-end benefit. For each:

| Item | Required |
|---|---|
| Priority and confidence | the measured contribution, and the uncertainty |
| Workload and target | reload or incremental; stage; query/table/operator |
| Observation | actual rows, time, bytes or calls — with the artifact path |
| Hypothesis | what avoidable work causes the measured cost |
| Experiment | the exact change, and its control |
| Both-workload impact | expected benefit, regression risk, write and maintenance cost |
| Correctness | blocking parity, frozen-score purity, deterministic partitions, golden/lineage equality |
| Decision | the success criterion **and** the rejection criterion |

A rejection criterion stated up front is what stops an experiment from being
re-interpreted as a success afterwards.

**Correctness is not negotiable for speed.** Scores are a pure function of
`(model_version, tf_snapshot_id, …)` — INV-SCORE — and a faster path that moves
a score is a wrong path. The historical record's own promotion rule was that
cluster precision must not decrease and recall may fall only fractionally, and
only in exchange for a large end-to-end win; nothing about a performance budget
relaxes the named invariants.

## Delivering an analysis

Separate reload and incremental timing tables; quality and output validation;
coverage limitations stated explicitly; and a ranked experiment ledger with
links to the queries. Distinguish **observations** from **hypotheses** — a
control/diagnostic pair locates a bottleneck, but claiming a gain needs
repeated unprofiled baseline-versus-candidate trials.

Artifacts may contain tenant data: keep analysis local, and do not send their
SQL or values to an external service. Save durable findings in `docs/`, and do
not change production schema or config, or launch an expensive run, when the
request was analysis.
