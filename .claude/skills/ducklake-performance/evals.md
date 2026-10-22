# Evals: ducklake-performance

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

Scenario 1 exists because this skill previously instructed running a script
that no longer exists. It is the regression test for that class of error.

## 1. The retired harness

**Query:** "Run the profiling campaign on the 1M workload and tell me where the
time goes."

Must:

- Say the Python/Splink campaign harness and its Compose-based driver are
  **retired and removed from the tree**, so there is no campaign to run.
- Name what does exist: `cargo bench -p dedupe-core`, the gated scale goldens,
  and `docs/measurements/*.json` as historical records.

Must not:

- Invent a campaign script, or instruct running anything under `benchmarks/`.

## 2. Comparing against the historical numbers

**Query:** "I measured the reload at 9 minutes. `docs/performance.md` says the
1M reload was 6 minutes, so we've regressed 50%. Confirm?"

Must:

- Refuse the comparison: those records are banner-marked **historical** and
  measured the **retired Python/Splink implementation**, so the difference is
  not a delta.
- Say a current baseline has to be measured.

Credit for also checking comparability (build, thread count, memory limit,
input hashes) before accepting any two numbers as comparable.

## 3. The index that cannot exist

**Query:** "`golden_records` lookups by `entity_id` are slow. Add an ART index
on it."

Must:

- Say ART indexes are **unsupported on DuckLake tables** and must not be
  proposed for one.
- Distinguish the local DuckDB **scratch** tables, where an ART index *is* a
  candidate for selective point lookups if the plan can use it.
- Offer the actual DuckLake levers: partitioning and sort-on-write, via
  row-group pruning.

## 4. The four-arm requirement

**Query:** "Partitioning `match_scores` by `match_key` should speed up the
duplicate queries. Let's do it."

Must:

- Require measurement first, and name the **four arms**: baseline, ordering
  alone, partitioning alone, both.
- Require write-side costs in the comparison: full-load writes, incremental
  deletes/updates, small-file growth, compaction.
- Flag `match_key` cardinality — a high-cardinality partition key produces many
  tiny files.

Credit for stating a rejection criterion up front.

## 5. Span arithmetic

**Query:** "The stage times add up to 14 minutes but the command took 9. Is the
ledger broken?"

Must:

- Say **nested durations must never be summed** — parent and child spans
  overlap, so the sum exceeding elapsed time is expected, not a bug.
- Add that CPU time can also exceed wall time under parallel execution.

Must not:

- Treat the discrepancy as unaccounted overhead to be attributed somewhere.

## 6. Speed at the cost of scores

**Query:** "If we recompute term frequency per batch instead of loading the
frozen snapshot, we skip a join and save time. Worth it?"

Must:

- Refuse: it violates **INV-SCORE / D4** (frozen TF, never recomputed from the
  corpus), so scores would depend on batch composition.
- State the principle that a faster path which moves a score is a wrong path.
