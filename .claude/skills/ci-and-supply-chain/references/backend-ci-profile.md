# Full backend CI profiling, November 9, 2026

Constraint: the standard GitHub-hosted runners that are free for public
repositories. No paid runners, self-hosting, or weakened correctness gates.
Local experiments precede pushing the implementation.

## Contents

- [Hosted baseline](#hosted-baseline)
- [Changes under test](#changes-under-test)
- [Local method](#local-method)
- [Initial paired results](#initial-paired-results)
- [First hosted result](#first-hosted-result)
- [Local validation](#local-validation)
- [Free runner experiment](#free-runner-experiment)
- [Server scheduling follow-up](#server-scheduling-follow-up)
- [Other options and coverage decisions](#other-options-and-coverage-decisions)

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

The selected level-3 Samply capture attributes approximately 11.4 CPU seconds
to the pipeline and 5.9 to corpus profiling, consistent with level 2. Full
captures, symbol sidecars, CPU-weighted folded stacks, and interactive SVG
flame graphs were retained locally under `/tmp/er-backend-profile-evidence`.
The SVGs focus on 28 pipeline stack levels and cap long labels; the Samply
captures retain the full stacks. The renderer is Brendan Gregg's FlameGraph
at `41fee1f99f9276008b7cd112fca19dc3ea84ac32`.

Cargo's emitted artifact metadata confirms the distinction: the er-cli test
has `opt_level=0`, line-table debug info, `debug_assertions=true` and
`overflow_checks=true`; only libduckdb-sys has `opt_level=3`. Its Rust assertions
and overflow checks are also still enabled. The old and selected executables'
complete integration-test identity lists match exactly.

## First hosted result

[PR run 42706135674](https://github.com/athvin/entity-resolution-engine/actions/runs/42706135674)
passed the complete backend gate in **4m42s**, creation to final job completion.
Against 8m11s–8m33s, this is **42.6–45.0% less elapsed time**. The refreshed PR run
[42708615045](https://github.com/athvin/entity-resolution-engine/actions/runs/42708615045)
and merged main run
[42709550901](https://github.com/athvin/entity-resolution-engine/actions/runs/42709550901)
took **4m57s and 4m56s**. A fresh old-code base run
[42706398325](https://github.com/athvin/entity-resolution-engine/actions/runs/42706398325)
took **8m56s**. These are complete gates, not content-cache skips; they establish
roughly 40–45% less elapsed time against the original baseline, not a p95 or a
guarantee for future cold dependency changes.

- All measured native C/C++ compilations were cache hits; the nightly cache
  did carry the optimized objects into the read-only PR run.
- Lake shard 3: 196 seconds wall, 73 compilation, 66.892 execution. Its slow
  incremental-equivalence test took 41.005 seconds rather than 218–226.
- The other lake shards executed in 41.596 / 25.817 / 16.152 seconds.
- Matching: 65 seconds compiling, 13.15 executing, versus about 104 / 94.89.
- Rest tests: 195 seconds wall, one 152-second build, about 32 seconds executing.
- The new critical path was Postgres m–z: 256 seconds wall, 105 compilation,
  122.55 execution, all 34 selected tests passing.

The token-rotation integration test deliberately waits **61 real seconds**
while testing a 60-second production refresh floor across real HTTP requests.
Its unit tests manipulate internal timestamps, but this integration test proves
the actual clock/network behavior. Keep that coverage and the production
interval. A blanket test timeout reduction or deleting its sleep would change
what it verifies. Better scheduling can overlap it with other work; a faster
runner can reduce compilation, but cannot eliminate this real-time floor.

The server-rest job still compiles two distinct package feature graphs (69 and
88 seconds). Combining them requires preserving package-specific filters and
doctests; blindly applying `--skip pg_` to every package is unsafe. It was not
the critical job in this run. Archive distribution and hardware comparisons
will determine whether more build orchestration is worthwhile.

## Local validation

After rebasing onto `8453c27d`, `make check` and `make check-e2e` passed.
The latter used isolated Postgres 16 and RustFS containers with the real lake,
Postgres, extension and S3 gates enabled. The revised selected-package runner
also passed the complete rest suite and all five extension tests using one
Cargo build. Its real output yielded 1,042 attributed test results; an identical
`--no-run` metadata pass reported every artifact fresh.

All 46 CI helper tests passed, including refusal of missing/ambiguous extension
artifacts, failed suites, changed experiment coverage, cold/broken native
caches, and unsafe target cleanup. The ARM64 release archives were checked
against their pinned SHA-256 values and all four executables ran in a local
Ubuntu 24.04 ARM64 container.

## Free runner experiment

GitHub lists `ubuntu-latest` and `ubuntu-24.04-arm` among its
[standard free public-repository runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
The optional `ci-runner-experiment.yml` compares three independent machines
per architecture. It is manual, main-only, and never a required check.
Production PR gates retain their current runner until measurements justify a
change and the remaining real-service tiers have passed on that architecture.
Require at least a 15% median improvement before considering a migration;
production images remain amd64, so Linux amd64 compatibility must stay covered.

Each machine primes the protected compiler cache with the actual selected
rest-suite runner, deletes only its disposable checkout's `rust/target`, then
times the same gate with a fresh target. Both phases include compilation, unit
and doc tests, and community-extension loading. The sample must have native
cache hits, zero native misses/cache errors, and exactly the priming phase's
attributed test identities. Artifact discovery occurs after timing and must
rebuild nothing. Compare compiler versions, commits, coverage digests, medians
and ranges across all six successful jobs before drawing a hardware conclusion.

The normal checkout path is deliberate: moving the target into a different
temporary directory changes native compiler-cache keys. Cleanup refuses local
or self-hosted execution and any target other than the checkout's Rust target.
Logs, `/usr/bin/time -v` resources, CPU/image metadata and raw cache statistics
are preserved as workflow artifacts. Tool and extension caches separate
architectures; unsupported platforms fail instead of falling back to x86 tools.

The first hardware comparison uses a separate Rust cache namespace because
`CARGO_TARGET_DIR` and `CARGO_TERM_COLOR` were explicitly set. The target path
was correct and native keys matched, but
[sccache 0.19.0 hashes nearly every `CARGO_*` variable](https://github.com/mozilla/sccache/blob/v0.19.0/src/compiler/rust.rs#L1839).
The three x86 priming phases consequently had 458 Rust misses; all three timed
samples had 492 Rust hits, 322 native hits and zero misses. Their warm timings
were 123.852 / 126.751 / 139.453 seconds for 1,051 identical test results.
This remains a valid warm hardware comparison, but the priming cost is not the
normal CI cache's cold cost. The follow-up removes both extra Cargo variables,
keeps the same default target path, strips terminal colour only when parsing
results, and rejects a nonstandard Cargo environment before building.

All six machines passed their tests, but two ARM jobs correctly refused to
publish a verdict because target identities differed between phases. Cargo's
stderr target headings had overtaken stdout results relayed by the selected
runner, assigning some `er-compile` results to its other target. Merging Cargo's
stdout and stderr before relaying fixes the ordering at its source; a subprocess
regression checks headings and test results stay ordered. This affects the
measurement attribution, not test execution or failure propagation.

The initial ARM samples were 136.819 / 141.176 / 143.661 seconds, with all native
and Rust compilations cache hits. These preliminary values show no advantage
over x86, but the failed identity checks mean this is not an accepted six-job
comparison. Repeat after the ordering and Cargo-environment fixes; retain x86
unless the complete comparison meets the stated improvement threshold.

## Server scheduling follow-up

The original server-rest job built `dedupe-cli`/`er-entity-diff`, then changed
the feature graph to build `er-server` again. Its server lib/bin/doc tests were
also already covered by the ordinary selected-package gate. The real-service
coverage unique to its integration binary was the non-Postgres Account read
case; every integration case can instead share the two server jobs that already
build that binary.

The follow-up runner discovers the complete integration test list from Cargo's
exact server artifact. Non-Postgres cases and a–l go to the early slice; the
complement goes to the late slice. The real token-rotation case starts in a
separate process alongside the early slice, retaining its database, real HTTP
requests, 61-second sleep, production interval and assertions. It owns its
Postgres database and dynamic HTTP listeners and shares no lake fixtures with
the other process. Other tests stay together to preserve their process-local
coordination. Every listed test belongs to exactly one group; a missing rotation
anchor, invalid list, missing artifact, unavailable services or failed process
fails the gate. Complete output lines are relayed per process so concurrent
libtest output cannot split a test identity.

Local original slices passed: a–l 61.523s, m–z 154.354s, non-Postgres 10.290s.
The proposed early slice passed in 138.807s on the busy workstation and 62.825s
on a repeat, with the rotation test's own 62.81s duration intact. The late slice
passed in 77.383s. These changing host loads do not establish a stable speedup;
hosted full-gate timing remains the acceptance measurement. Coverage remains
135 passed and the same two existing ignored tests across the 137 integration
identities. After complete-line relaying, both real slices passed again in
62.863s / 50.842s and their exact executed name/status lists matched the original
137 results. The helper's subprocess test also proves that both groups start
before either waits; compile, artifact and child failures are exercised.

This removes the second server-rest build while keeping `dedupe-cli` and
`er-entity-diff` unfiltered: their database tests do not all have `pg_` names.
It does not clone initialized schemas or shorten the token refresh interval.

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
  identities before this native optimization. Including producer/transfer cost, its median was 498 versus
  464 seconds (7.33% slower). A fresh optimized comparison
  [42713537379](https://github.com/athvin/entity-resolution-engine/actions/runs/42713537379)
  passed all 45 jobs and matched exact test identities. Five baseline/archive
  critical paths were 172/202, 187/221, 169/207, 178/188 and 180/178 seconds:
  **median 178 versus 202 seconds, archive 13.48% slower**. This includes producer,
  transfer and slowest consumer costs, excluding unrelated queue contention.
  Retain separate cached builds. See build-cost.md.
- Replacing every Cargo test invocation with nextest was also measured slower
  for short tests in large statically linked executables. Keep nextest where
  it supplies useful lake-test partitioning; preserve the Postgres suite's
  in-process coordination.
- Do not clone a preinitialized database for all Postgres tests: that would
  stop exercising bootstrap. A local ABBA comparison recreated Postgres 16
  before each arm and ran all 64 `pg_` tests with four workers. Every arm
  passed, with fsync, synchronous commit and full-page writes all on.
  Disk / tmpfs / tmpfs / disk took 269.874 / 283.605 / 124.707 / 168.182 seconds.
  Concurrent work made the host load vary substantially; tmpfs did not show
  a repeatable benefit. Retain ordinary storage rather than adopting a change
  from this noisy comparison.
- Free ARM64 is measured by the opt-in experiment above. macOS/Windows would
  also change the platform and service-container support; neither is a
  substitute for the Linux deployment compatibility gate.

Cold native compilation and warm execution are separate outcomes. A profile
change invalidates native compiler-cache entries; PRs must not gain write access
to the protected main cache to conceal that first-run cost. Measure a warm
hosted run after main populates its cache before claiming a new CI duration.
