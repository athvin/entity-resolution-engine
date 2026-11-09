# Full backend CI profiling, November 9, 2026

Constraint: the standard GitHub-hosted runners that are free for public
repositories. No paid runners, self-hosting, or weakened correctness gates.
Local experiments precede pushing the implementation.

## Hosted baseline

Two successful full runs after the setup/cache improvements:

- [42642270239](https://github.com/athvin/entity-resolution-engine/actions/runs/42642270239),
  `f7fb8b4c`: approximately 8m33 from creation to the last job completing.
- [42639867651](https://github.com/athvin/entity-resolution-engine/actions/runs/42639867651),
  `06bc5f0d`: approximately 8m11.

Times below are job wall time / summed Cargo compilation / test execution,
in seconds. Parallel jobs and overlapping tests must not be added to claim
workflow elapsed time.

| Job | First run | Second run |
| --- | --- | --- |
| Lake shard 3 | 487 / 87 / 360 | 464 / 86 / 336 |
| Lake shard 1 | 316 / 92 / 180 | 430 / 88 / 303 |
| Lake shard 2 | 281 / 96 / 141 | 304 / 85 / 177 |
| Lake shard 4 | 270 / 88 / 140 | 262 / 84 / 141 |
| Test rest | 414 / 250 / 143 | 381 / 245 / 114 |
| Server pg a–l | 394 / 145 / 218 | 361 / 112 / 222 |
| Server pg m–z | 393 / 139 / 222 | 404 / 121 / 248 |

The slowest single test,
`lake_e2e::an_incremental_batch_lands_where_a_full_rerun_would`, took
226 / 218 seconds. It compares pair identities, bit-exact probabilities,
partitions, and fixture ground truth across incremental and full processing.
Keep it: agreement between two paths alone would not catch both being wrong.

The critical job's setup was about 23 seconds, including its service container.
Further setup tuning cannot remove several minutes of database execution.
The Postgres tier also contains lake-backed tests; its name does not mean all
its time is spent in Postgres.

## Changes under test

1. Optimize only `libduckdb-sys` in the dev profile, inherited by tests.
   Its bundled C++ build receives Cargo's `OPT_LEVEL`; the existing default
   compiled DuckDB with `-O0`. Our Rust code retains its debug assertions,
   overflow checks, and test line tables. The dependency's existing
   `debug = false` and native `NDEBUG` setting are unchanged.
2. Run extension tests from the `dedupe-core` integration executable emitted
   by the selected-package build. Re-running Cargo with only `-p dedupe-core`
   changes feature unification: the observed extra build took 65 seconds to
   run five tests in 0.25 seconds. Cargo's JSON artifacts identify the executable;
   missing/ambiguous artifacts, a missing community-extension test, test-suite
   failure, and extension failure all fail the gate. Only that executable gets
   `DEDUPE_E2E_EXTENSIONS=1`; other crates' gated tests are not silently enabled.

## Local method

Base `3e95a13de0b62426128c6855eb544c88011fa20e`, isolated worktree;
macOS arm64, eight logical CPUs, 24 GiB RAM, rustc 1.99.0.
GitHub baseline uses Linux x86-64: local seconds are **not** a forecast of
hosted wall time. Compare paired relative changes, then observe hosted jobs.

Preserve each compiled executable before changing profiles. Run identical
tests with identical fixture data, extensions and environment, alternating
the arms with compilation stopped. For in-process `lake_e2e` tests, copying
the executable preserves the whole statically linked engine. Tests that spawn
`CARGO_BIN_EXE_er` also require preserving that CLI; do not accidentally compare
an old harness invoking the new CLI.

Example build override before changing the manifest:

```sh
cd rust
cargo test -p er-cli --test main --no-run --message-format=json \
  --config 'profile.test.package.libduckdb-sys.opt-level=2'
DEDUPE_E2E_DUCKLAKE=1 <saved-test-executable> \
  lake_e2e::an_incremental_batch_lands_where_a_full_rerun_would --exact
```

Local profiling uses Samply's CPU samples, retaining its symbol sidecar.
An initial diagnostic capture used an older binary while compilation competed
for CPU: useful for locating repeated corpus profiling/query planning, **not**
an end-to-end performance comparison. Only idle-build, current-source runs
belong in the timing comparison.

## Initial paired results

The three current-source process timings were 70.090 / 66.439 / 66.441 seconds
at the original setting, and 13.213 / 11.369 / 11.378 at optimization level 2.
Medians: **66.441 → 11.378 seconds, 82.9% less elapsed time (5.84×)**.
The first separate baseline warm-up was 73.191 seconds and is excluded from
these paired results. Every run passed the same assertions.

The integration executable shrank from approximately 192 to 93 MiB.
Independent current-source Samply captures attributed about 65.5 → 11.1 CPU
seconds to the pipeline, and 32.2 → 5.8 to automatic corpus profiling. These
are sampled inclusive call-tree costs: the child is included in its parent,
and they must not be added. SQL planning/execution dominates the captures.
They are diagnostic evidence, separate from the unprofiled timing trials.

The optimization-level sweep retained the same source and assertions:

| Native level | Three process times (seconds) | Median |
| --- | --- | --- |
| 0 (original) | 70.090 / 66.439 / 66.441 | 66.441 |
| 1 | 13.586 / 11.639 / 11.517 | 11.639 |
| 2 | 13.213 / 11.369 / 11.378 | 11.378 |
| 3 (selected) | 13.368 / 11.254 / 11.294 | 11.294 |

Levels 1–3 perform similarly on this fixture. Select level 3 to match the
nightly benchmark's native compilation flags and existing compiler cache.
[That morning's benchmark job](https://github.com/athvin/entity-resolution-engine/actions/runs/42577173686/job/129027451461)
reported all 305 C++ compilations as cache hits. This establishes that those
objects exist; the PR must still verify that its native cache keys match.

Local native rebuilds plus relinking took approximately 158 / 168 / 202 seconds
at levels 1 / 2 / 3 with Rust dependencies already available. These are separate
compilations, not clean whole-workspace builds. Level 3 does cost more cold;
cache reuse, unchanged debug/test checks, and faster execution are the intended
tradeoff. The original seven-minute local build also rebuilt Rust dependencies
and is not a valid cold-build comparator.

The complete `lake_e2e::` module, with four libtest workers and all 52 tests,
passed in **343.506 → 76.350 seconds** (77.8% less elapsed time, 4.50×).
This is one complete comparison per arm after the repeated single-test trials;
not a p95 estimate. Both returned zero with 52 passed, zero failed and zero
ignored. The selected level-3 integration executable is about 99 MiB.

```sh
DEDUPE_E2E_DUCKLAKE=1 <saved-test-executable> lake_e2e:: --test-threads=4
```

## Other options and coverage decisions

- Package selection already follows reverse Cargo dependencies, with full
  fallback for unknown inputs. Full CI must still test the complete graph.
- Scale/performance jobs are already nightly/manual. Unit, schema, golden,
  isolation, migration, incremental-equivalence and real-service tests protect
  different failure modes; removing these would trade correctness for speed.
- The four lake shards are imbalanced. Reordering tests within the same shard
  cannot remove its total work: a longest-first simulation of the first run
  only reduces roughly 360 to 338 seconds with unchanged test durations.
  Cross-shard balancing can help, but first reduce the long individual test.
- More shards duplicate compilation and consume the same account concurrency.
  A 226-second individual test sets a floor regardless of the shard count.
- A prior compile-once/archive experiment covered 45 jobs and matched test
  identities. Including producer/transfer cost, its median was 498 versus
  464 seconds (7.33% slower); retain separate cached builds. See build-cost.md.
- Replacing every Cargo test invocation with nextest was also measured slower
  for short tests in large statically linked executables. Keep nextest where
  it supplies useful lake-test partitioning; preserve the Postgres suite's
  in-process coordination.
- Do not clone a preinitialized database for all Postgres tests: that would
  stop exercising bootstrap. A temporary filesystem can be measured without
  disabling fsync, synchronous commit, or full-page writes.
- Free Linux ARM64 runners are another platform, not a drop-in label change:
  the current verified tool installer explicitly supports Linux x86-64 only,
  and compiler/extension caches are architecture-specific. This investigation
  keeps the established runner architecture while addressing measured costs.

Cold native compilation and warm execution are separate outcomes. A profile
change invalidates native compiler-cache entries; PRs must not gain write access
to the protected main cache to conceal that first-run cost. Measure a warm
hosted run after main populates its cache before claiming a new CI duration.
