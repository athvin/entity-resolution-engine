---
name: ducklake-performance
description: The measurement discipline and storage-tuning rules for DuckDB/DuckLake performance work in this repo — how to read span and resource evidence without over-claiming, what the committed historical measurements do and do not license you to conclude, which layout levers exist and the experiment matrix a layout proposal must cover, and the live Rust benchmark surface. Use when a stage or query is slow, when proposing a partition, sort, index or extension change, when reading a profiling artifact or a measurement record, or when comparing a candidate against a baseline. For the ATTACH options, maintenance ordering, and the partitioning and sort-on-write syntax themselves see ducklake-lakehouse — the mechanism is there, whether to change it is here; for matching internals see er-matching; for running the stage at all see er-engine-ops; for emitting the span or metric the evidence comes from see observability.
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
| the 50k scale goldens | gated, and never a per-PR gate — which jobs run them, and when, is in the ci-and-supply-chain skill's generated required-checks reference |

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

[references/measurement-discipline.md](references/measurement-discipline.md) is
canonical here — the full checklist, the resource-evidence rules, and the
comparability keys that decide whether two runs are even comparable. These three
are repeated deliberately, because they decide most analyses:

- **Never sum nested durations.** Parent and child spans overlap, so a sum that
  exceeds the command's own runtime is expected rather than unaccounted
  overhead — as is CPU time exceeding wall time under parallel execution.
- **Use unprofiled control time as the end-to-end result.** Profiling has
  overhead; report it separately, and never compare a profiled time against a
  prior unprofiled one.
- **Missing evidence is not zero.** An unprofiled thread is not an idle one, and
  a local object-store stand-in establishes CPU and plan shape, never remote
  request latency or bandwidth.

Setup, data generation, validation and full-rescore references sit **outside**
workload timing. Decide that boundary before measuring, not after seeing the
numbers.

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
- **An expensive query** — the plan's estimated-versus-actual cardinalities,
  rows scanned versus returned, and repeated lake scans. Link every claim to a
  specific query or operator, never to the stage as a whole.

For the matching stages specifically, blocking decides the entire candidate
population before any of them runs: `__dedupe__blocked_id_pairs` carries one
`UNION ALL` arm per rule, and an array-column (exploding) rule can inflate the
candidate count on its own. Tune blocking before anything downstream of it —
the er-matching skill carries the stage table and the `exploding` golden
fixture.

## Layout: the levers and the experiment matrix

DuckLake has **no secondary indexes** (`CREATE INDEX` is rejected), so the
levers are partitioning and sort-on-write. Do **not** propose an ART index on a
DuckLake table; that is a local-DuckDB-scratch option only.

A layout proposal must test **four** arms, not two:

```
baseline │ ordering alone │ partitioning alone │ both
```

because the arms interact, and because a layout that reads faster can write
slower. [references/tuning.md](references/tuning.md) is canonical for layout:
the storage boundaries, the pruning mechanisms that actually apply, and the
per-arm cost list a proposal must report.

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
