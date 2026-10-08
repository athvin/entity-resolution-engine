# Performance

*The consolidated performance record: what the pipeline costs today, what drives
that cost, how it got here, what was tried and rejected, and what the evidence says
to do next. Consolidated 2026-10-03 from nine measurement reports; the full
per-stage tables and campaign methodology live in git history, each committed
campaign's machine-readable record is under [measurements/](measurements/) (the
work-in-progress thread-scaling campaign has none — see
[Measurement records](#measurement-records)), and raw artifacts
are git-ignored under `artifacts/`.*

## The numbers that matter

The standard workload is **1M synthetic records (400k people), seed 42,
`configs/default.yaml`**, measured in the local envelope — 2 CPUs, 10 GiB
container, 2 DuckDB threads, 4 GB DuckDB memory limit — on an ARM64 Docker host.
Times are complete processing (CLI start to finish, training included);
generation, setup, validation and teardown are excluded.

| Workload | Time | Basis |
|---|---:|---|
| 1M full reload | **91.70s median** (~10,900 records/s) | 3 trials, 2026-09-23 |
| 100K incremental onto the 1M lake | **39.48s median** | 3 trials, 2026-09-23 |
| 10M full reload | **1h 46m 17s** | 1 trial, 2026-09-23 |
| 100K incremental onto the 10M lake | **2m 50s** | same trial |
| 10M full reload at 6 threads | **1h 04m** (−40%) | 1 trial, work in progress — not a committed baseline |

Resource shape at 10M, same envelope: peak container memory **9.37 GiB**, peak
*live* DuckDB spill **141.48 GiB**. Ten million records fit in a 10 GiB container
because DuckDB spills instead of growing its working set — scale costs fast
scratch disk, not RAM. ([infrastructure.md](infrastructure.md) §6.5 sizes the
cloud runner classes on exactly these two numbers.)

## Four findings that drive everything

1. **Training dominates at scale.** 78–80% of the 10M reload is model training,
   driven by the surname/postcode EM rule's **491,624,467 pairs**. At 1M it is
   ~11%. Narrowing *prediction* blocking cannot remove this — the training rule is
   a different pair population.
2. **Memory is nearly flat in corpus size; spill is not.** 1M peaks ~4 GiB and 10M
   peaks 9.37 GiB at the same 4 GB DuckDB limit, while spill reaches 141 GiB.
   Capacity planning is vCPU plus scratch disk, not memory tiers.
3. **DuckDB threads are the wall-clock lever** (work in progress — one pass per
   arm, no machine-readable record). 10M, 2→6 threads at the same 4 GB DuckDB
   limit: **−40%**, peak memory flat (9.14 GiB). 1M: **−22%**, but the 6-thread
   arm also raised the DuckDB limit 4→6 GB (peak 4.06→4.26 GiB), so it is not a
   pure-thread comparison. Golden/lineage counts were identical across arms (at
   1M, every quality metric too); no byte-level output comparison was recorded.
   Training and matching scale near-linearly; ingestion is I/O-bound;
   **reconciliation is a Python graph step that does not parallelize** (+3% at
   1M, +14% at 10M) and becomes the largest 1M stage. The curve above 6 threads is unmeasured
   (infrastructure.md §18.2).
4. **Incremental is cheap and provably equivalent.** 100K records onto the 10M
   lake took 2m 50s against the 1h 46m reload, and every campaign gates on
   incremental ≡ frozen-model full re-resolution — the 10M check compared all
   8,621,617 stored pairs with **zero mismatches and zero probability
   difference**.

## How it got here

The September 2026 optimization arc took the 1M reload from 25½ minutes to ~92
seconds. Medians come from separate campaigns at the same envelope, so the rows
are milestones, not one continuous series:

| Change | 1M reload | What moved |
|---|---:|---|
| Starting point (2026-09-21) | 1,532s | Matching was 1,192s of it — row-at-a-time review-queue persistence |
| Batched persistence + bounded staging | 208s | Matching **62× faster**; reconciliation −73%; peak memory 8.5 → 4.5 GiB. The historical 100k campaign cut reconciliation **−91%** |
| SQL pushdown of ingest/review/reconcile data flows | 169s | The isolated reconcile kernel ran **52× faster**, the whole pipeline only −7% — kernel wins do not transfer proportionally |
| Splink 5 migration + MARK-join removal in standardization | 90.8s median (**−42%**) | Standardization 83 → 21s at 1M and **−84% at 10M** (whole 10M run −9.8%). The old incremental batch discovery was quadratic — stopped at >300s versus 30.8s after |
| Profile-guided tuning: event-encoding reuse, 8,192-row staging pages, captured current-score table | 91.7s (−2.5%) | Reconciliation −9.7%; diminishing returns at 1M |

## Quality status

- **Determinism holds everywhere it was checked**: thread counts and prediction
  execution settings do not change scores, and the Splink 4→5 migration
  reproduced identical quality metrics, partitions and output counts at 100k, 1M
  and 10M.
- **Precision at scale is the open accuracy problem, and performance work has not
  moved it.** Baseline profile: 1M cluster precision 90.34% / recall 98.70%; 10M
  **82.74% / 97.91%** with **1,633,894 false-positive cluster pairs**.
- **The EM pair-target experiment is promising and unpromoted.** `max_pairs: 1M`
  cut training **−29.4%** and the load −5.5% in a single hard-profile comparison,
  with quality equal or marginally better. Promotion gates (stated in full in
  DesignDoc §10.6, enforced by `benchmarks/acceptance.py`): three 10M repeats,
  every held-out dataset evaluated independently, cluster precision
  non-decreasing, any recall loss (max 0.01pp) must buy ≥20% end-to-end time in
  the target workload, and no repeatable slowdown in another workload.
- **Blocking-rule narrowing was rejected**: removing or narrowing the broad
  prediction rule dropped 71–78% of candidates but cost 0.40–0.71pp of cluster
  recall against the 0.01pp budget.
- MinHash and BGE/ONNX candidate-key providers are validated as implementation
  paths on a 1k smoke only; nothing is activated in production.

## Tried and rejected

Recorded so nobody retries these without new evidence:

- **Storage layouts** — sorted writes, source partitions, and both together: no
  total write+read win in either workload, and the batch predicate sat above the
  scan so file pruning never actually happened. Raw-record DDL is unchanged.
- **Prediction chunking (2×2, 4×4) and Parquet scratch** — chunking added time at
  this scale; default stays table storage, one chunk per side.
- **Survivorship alternatives** — the bounded top-two aggregate was slower; the
  shared `lead` window showed no consistent benefit. The production macro is
  unchanged.
- **Splink native hash sampling and u early stopping** — measured precision or
  recall regressions; neither became a default.

## Open backlog, evidence-linked

1. **The 10M training fan-out** — run the EM-cap promotion campaign through its
   gates. The only candidate with an order-of-magnitude target on the dominant
   cost.
2. **Reconciliation / label propagation** — the largest 1M stage after the
   matching fix, and it anti-scales with threads. A frontier/changed-label reuse
   experiment must preserve deterministic minimum labels, never-cut parity and
   non-convergence behavior, and must include long/adversarial-chain tests — the
   inputs where an incremental frontier diverges from fixpoint propagation.
3. **dbt startup and compile overhead** — CLI import spans alone total 6.85s
   across the reload's eight commands and 3.41s across the incremental's four.
   These are instrumented diagnostic wall spans, already counted inside their
   enclosing command times — an upper bound on the opportunity, not a measured
   gain. Any reuse must preserve tenant configuration, run variables and
   compiled SQL.
4. **An incremental batch predicate that permits file pruning** while preserving
   batch-ID checkpoint and replay semantics.
5. **The thread-scaling knee above 6** — sets the top of the cloud runner class
   ladder (infrastructure.md §18.2).

## Measuring

Entry points:

```sh
make benchmark                      # 1M initial load (alias: benchmark-1m); BENCHMARK_REPEAT=3 for medians
make benchmark-workloads            # + separately timed 10k delivery and correction
make benchmark-10m                  # 10M capacity run, local-fitted limits
make profile-workloads              # 1M reload + 100K incremental, diagnostic campaign
```

`benchmarks/full_pipeline.py` is the underlying runner: `--scale {smoke,10k,100k,1m,10m}`,
`--local` (fits limits to the host without shrinking the corpus), immutable
`--image`/`--image-source`, `--corpus-root` to reuse generated inputs,
`--with-incremental`, `--profile`, `--keep-failed` (retain a failed stack for
diagnosis; tear it down afterwards with the `retained_project`/`retained_image`
names recorded in `manifest.json`). Two `--local` gotchas: limits are derived
from the host, so verify a run's recorded CPU, container-memory and DuckDB
limits before comparing it against a recorded trial; and the free-disk check is
a 2 GiB startup guard, not a prediction — a 10M reload spills ~141 GiB.
`benchmarks/performance_campaign.py` runs the
arm-based training/blocking experiments; `benchmarks/tuning_report.py` compares
baseline/candidate trial sets; `benchmarks/build_performance_image.py` plus
`benchmarks/performance.py` run frozen-image A/B comparisons.

The rules every retained number followed: frozen images and recorded source
hashes; fresh stacks per trial on an otherwise idle host; **unprofiled times
only** (diagnostic collection adds 42–49% overhead and is never timed as
evidence); equivalence gates — exact classifications, partitions, golden values
and lineage; pair probabilities within `1e-10`; independently learned parameters
within `1e-12`; model/TF bytes unchanged within a trial; failed trials stay
failed. The historical A/B campaigns additionally alternated baseline/candidate
order; the 2026-09-23 tuning trials ran sequentially and reuse one profiling
control (noted in that record's limitations), and single-trial numbers say so in
their basis column. In the recorded fingerprints the full `config_hash`
legitimately differs between trials — each trial runs in a fresh random tenant
namespace that flows into storage paths — while `semantic_config_sha256` hashes
the namespace-independent configuration and is the comparability key.
Python-side work that
deliberately stays out of SQL is inventoried in
[python-processing-exceptions.md](python-processing-exceptions.md); trace
semantics are in [profiling.md](profiling.md); migrating an existing lake across
the Splink 4→5 boundary is in the
[runbook](runbook.md#migrating-an-existing-lake-to-splink-5).

## Benchmark baselines

The weekly/manual smoke workflow currently has no measured baseline and reports
`NO_BASELINE`. `10k`, `100k` and `1m` remain defined workloads; their dispatch
options stay disabled until reviewed measurements exist.

```sh
COMPOSE_PROJECT_NAME=er-benchmark bash scripts/ci/bench.sh
uv run python benchmarks/report.py \
  --compare artifacts/bench/latest.json --scale smoke \
  --baselines-dir benchmarks/baselines --write-baseline
```

Review quality, variation and the environment fingerprint before promoting; the
command refuses a `NON_COMPARABLE` run. Commit the generated baseline with its
`baseline_committed` flag in `benchmarks/scales.yaml` (plus `dispatchable` for
larger scales). `OK` means within threshold, `REGRESSION` means a comparable
phase exceeded it, `NON_COMPARABLE` cannot be judged, `NO_BASELINE` had nothing
to compare. Historical optimization results above are separate from these CI
baselines.

## Measurement records

| Campaign | Machine-readable record |
|---|---|
| Splink 5 migration (2026-09-22) | [measurements/splink5-20260922.json](measurements/splink5-20260922.json) |
| 1M/100K profiling baseline (2026-09-23) | [measurements/workload-profiling-20260923.json](measurements/workload-profiling-20260923.json) |
| Profile-guided tuning (2026-09-23) | [measurements/workload-tuning-20260923.json](measurements/workload-tuning-20260923.json) |
| 10M/100K scale test (2026-09-23) | [measurements/workload-10m-100k-20260923.json](measurements/workload-10m-100k-20260923.json) |
| Training/blocking quality screen (2026-09-23) | [measurements/performance-screening-20260923.json](measurements/performance-screening-20260923.json) |
| EM-cap full-load confirmation (2026-09-23) | [measurements/training-full-load-20260923.json](measurements/training-full-load-20260923.json) |

The work-in-progress thread-scaling campaign (2026-09-29, PR #54) has no machine-readable
record — its raw outputs were git-ignored under `artifacts/bench/threads6-*` on
one developer machine. Its report survives in git history as
`docs/performance-thread-scaling.md` at `9090b33^`; the 2-thread 10M reference it
compared against (6,458s) is the Splink-5 campaign's run in
[measurements/splink5-20260922.json](measurements/splink5-20260922.json).
