# Build cost: where Rust CI time goes

Every timeout in `rust.yml` carries the measurement that justifies it, which is
the right place for a *timeout*. What that leaves out is the cross-job picture —
which job is the critical path, how far cold diverges from warm, and why cold
happens at all. This file is that picture, and the decision log for changes made
against it.

## Contents

- The measured baseline (2026-10-22)
- Why cold happens: the Actions cache budget
- The levers, and which were taken
- Rejected: headstart
- Rejected: larger runners, for now
- Rejected: removing sccache
- Escalation: sccache on S3

## The measured baseline (2026-10-22)

Two `rust` runs on `main`, one hour apart. Nothing about the diffs explains the
difference — the first missed the cache, the second hit it.

| Job | cold (run `39717477212`) | warm (run `39739127156`) |
|---|---|---|
| **fmt + clippy + docs** | **42m27** | 14m15 |
| DuckLake + extensions e2e | 27m55 | 12m48 |
| benchmarks (compile) | 14m38 | 6m55 |
| test | 14m22 | 9m29 |
| scale golden (50k) | 9m12 | 3m02 |
| cargo-deny | 30s | 31s |
| agent skills (lint) | 5s | 5s |
| coverage (llvm-cov) | skipped | 23m26 |
| **workflow wall clock** | **42m38** | 23m27 |

`fmt + clippy + docs` is the critical path, and it is four independent
~10-minute steps run **serially**:

| Step | cold | warm |
|---|---|---|
| `cargo fmt --all --check` | 1s | 5s |
| `cargo clippy --workspace --all-targets` | 10m32 | 6m37 |
| `er-entity verify` | 10m39 | 2m30 |
| `cargo doc` (`RUSTDOCFLAGS: -D warnings`) | 9m20 | 1m55 |
| `skills-check.sh check` | 1s | 1s |
| OpenAPI breaking-change gate (**advisory**, `\|\| true`) | 10m34 | 2m15 |
| rust-cache save (post-job) | 1m06 | 1s |

Two things follow. The advisory gate — a check that *cannot* fail the build — was
costing 10m34 of critical path. And cold-vs-warm is a 3× swing on the one job
that gates every merge, so **reducing how often cold happens is worth more than
any per-step optimisation.**

Elsewhere: `test` spends 2m33 in the disk-reclaim step before compiling, and
10m47 in `cargo test --workspace`. `ducklake-e2e` splits 16m36 / 9m47 between its
four-crate test and the rapidfuzz extension test. `coverage` is 22m10 of
`llvm-cov` alone, nightly-only, and deliberately opts out of sccache and mold
(`rust.yml:358-360`).

## Why cold happens: the Actions cache budget

GitHub allows **10 GB of Actions cache per repository** and evicts LRU past it.
Measured 2026-10-22, before any change here:

```
active_caches_size_in_bytes: 10,653,007,178   # 9.92 GiB / 10.65 GB
active_caches_count:         2,594

sccache              ~2,706 entries   4,410 MiB    # 99% of entries, 44% of bytes
v0-rust-rust              2 entries   3,017 MiB    # what 5 compile jobs restore from
v0-rust-coverage          1 entry     1,885 MiB    # for a nightly-only job
pnpm-store                2 entries     375 MiB
next-build                2 entries     201 MiB
buildkit-blob             5 entries     161 MiB    # images.yml, type=gha,mode=max
duckdb-extensions         1 entry        43 MiB
```

Whether that is *over* the ceiling or merely *at* it depends on whether GitHub
means 10 GB or 10 GiB, and the enumerated total (9.85 GiB) trails the reported one
because entries churn during enumeration. The symptom is not ambiguous: cold runs
happen routinely, and only two `v0-rust-rust` entries survive at any time.

**The mechanism is entry count, not entry size.** sccache's GHA backend writes
**one Actions-cache entry per compilation object**. Nothing caps it. Those ~2,700
small entries are, collectively, what pushes the two 1.3–1.7 GB rust-cache
entries past the ceiling — the same entries whose presence is the whole difference
between a 14-minute and a 42-minute gate. sccache is evicting the cache it was
added to complement.

That it grows monotonically is not inferred. Re-read four hours later, with no
dependency change in between:

```
active_caches_size_in_bytes: 10,682,318,008   # +29,310,830 in four hours
sccache              ~2,945 entries   4,571 MiB   # +239 entries, +161 MiB
```

The repository does not get *further* over the ceiling, because eviction is what
absorbs the growth. It is the rust-cache entry that pays.

Two structural contributors, both fixed:

1. **No rust-cache use set `save-if`.** All five compile jobs wrote the same
   `shared-key: rust`, and PR branches minted their own entries. `lint` is now
   the single writer, and only from `main`.
2. **sccache grew without bound**, since nothing pruned it. Capped nightly by
   `cache-prune`.

A third looked structural and is not. **`coverage`'s separate 1,885 MiB entry** —
19% of the whole budget, for a nightly, non-required job whose own comment says
instrumented builds "don't cache usefully" — was left alone deliberately.
Dropping it makes that job permanently cold, and its `llvm-cov` step already
measures **22m10 warm against a 60-minute timeout**: llvm-cov owns `RUSTFLAGS`,
so every unit including dependencies rebuilds, and `shared-key: rust` cannot
substitute. That trades 1.8 GiB for a timeout failure. It is also unnecessary —
`[profile.test]` inherits the dep setting below, so the entry shrinks on its own,
the bulk of it having been the duckdb rlib.

A fourth contributor is not CI's: `rust/Cargo.toml` carried **no `[profile.*]`
section at all**, so the debug-info saving measured at `rust.yml:179-194` —
duckdb rlib **1480 MB → 240 MB**, whole build **12631 MB → 7234 MB** — applied
only to the one job that set it as env. A smaller `target` is also a smaller
rust-cache entry, so making the profile real buys cache headroom twice.

## The levers, and which were taken

| Lever | Taken | Why |
|---|---|---|
| Real `[profile.*]` in `rust/Cargo.toml` | yes | 1.9× less build output for every job and every laptop, not just `test` |
| `save-if` so one job writes each key | yes | stops five jobs racing the same key and PR branches minting entries |
| Drop `coverage`'s cache save | **no** | frees 1.8 GiB but makes a 22m10-warm job permanently cold against a 60-min timeout; the profile change shrinks the entry instead |
| Prune sccache to a budget, nightly | yes | caps the entry count that does the evicting |
| `sccache --show-stats` in each job | yes | the hit rate was never observable; it is the input to every later decision here |
| Split the serial `fmt + clippy + docs` | yes | four independent ~10m steps; the advisory one was pure critical-path tax |
| cargo-nextest | no | build time dominates execution time here; revisit after the above |
| `.cargo/config.toml` for local mold/sccache | no | helps laptops, not CI; worth doing, not yet scoped |
| cargo-chef + cache mounts in `rust/Dockerfile` | no | `COPY . .` + one `cargo build --release` means any source change rebuilds DuckDB in release mode; real, separate |
| Larger runners | no | see below |
| headstart | no | see below |

## Rejected: headstart

**Decision.** Do not adopt [headstart](https://github.com/PowderworksCode/headstart)
— a patched rustc + cargo pair that lets a dependent crate start on its
dependency's *interface* metadata (`.early-rmeta`) instead of waiting for its
function bodies.

**Context.** It is a serious piece of work: up to 54% faster `cargo check` and
42% faster `cargo build` across 13 real projects, with correctness evidence well
past prototype grade. Four facts, all from its own documentation, rule it out
*for this repo as configured*:

- **It is incompatible with the sccache already in place.** Its README: "sccache,
  or any `rustc-wrapper` that holds back rustc's output until it exits, hides the
  early metadata and **so turns headstart off**." Every compile job sets
  `RUSTC_WRAPPER: sccache` (`rust.yml:128, 178, 259, 316, 337`).
- **4 cores is its documented worst case.** Its gain "comes from cores the build
  would leave idle." Its `docs/readiness.md`: on 4 cores wide builds are "within
  noise of even", and a saturated machine spends **3–7% more rustc CPU**. Every
  job here is `ubuntu-latest` — 4 vCPU.
- **No prebuilt toolchain exists** (the repo has zero releases). `setup.sh` yields
  a development rustc ~2× slower than nightly; `release.sh` is ~1h on 16 cores and
  25 GB of disk. Runners have ~14 GB free *after* the reclaim step, so it cannot
  be built in CI — it would be a separately built, separately maintained artifact.
- **Toolchain mismatch against the gate.** `rust-toolchain.toml` is
  `channel = "stable"`; headstart patches rustc *master*. The two heaviest gate
  steps are `cargo clippy -D warnings` and `cargo doc` with
  `RUSTDOCFLAGS: -D warnings`, both sensitive to compiler lint surface, and
  clippy-driver and rustdoc on the patched tree are untested. So is macOS, since
  its locking rewrite.

It also does nothing for the bundled DuckDB C++ compile, which this repo's own
timeout comments name as the cold-build cost driver — that is cc-rs, not rustc
crate scheduling.

**The steel-man, recorded so it need not be re-derived.** The *shape* of this
workspace is exactly what headstart fixes. 28 first-party crates form a
dependency chain, and a chain compiles as a staircase with cores idle beside it;
on a warm cache the external dependencies are already built, so that staircase is
most of what the 6m37 warm clippy still does. codex-rs, the same shape, checks 14%
faster on 4 cores.

**Revisit trigger.** Either of:

1. The heavy Rust jobs move to ≥8-core runners **and** `sccache --show-stats`
   shows sccache is not earning its keep — both conditions, since headstart
   requires its removal and benefits from the cores.
2. The patches land upstream behind a flag on a channel this repo can pin, which
   removes the maintained-toolchain and lint-surface objections at once.

**Date.** 2026-10-22.

## Rejected: larger runners, for now

**Decision.** Stay on `ubuntu-latest` until the cache and profile fixes above have
been measured.

**Context.** Larger GitHub-hosted runners are the one lever that multiplies every
job at once, and they would also relieve the disk wall that forced the reclaim
step and the debug-profile hunt. They cost roughly 2× per minute at 8 cores. The
case for deferring is that a 42-minute job that should be 14 is not a compute
problem, and buying cores to paper over a blown cache budget would hide the
actual fault while still paying for it on every run.

**Rejected alternative.** Moving `lint`, `test` and `ducklake-e2e` to
`ubuntu-latest-8-core` now. Reconsider once the baseline above has been
re-measured post-change — and note that it is also precondition 1 for a headstart
revisit.

**Date.** 2026-10-22.

## Rejected: removing sccache

**Decision.** Keep sccache; cap it instead.

**Context.** Its 4.3 GiB looks like pure duplication of rust-cache's target dir,
and dropping it would free the budget outright. But the two caches fail in
different ways, and sccache covers the case rust-cache cannot: rust-cache keys on
`Cargo.lock`, so **any** dependency change misses it wholesale, while sccache is
content-addressed per compilation unit and still hits for every crate the bump did
not touch. `docs/frontend-testing.md:382-388` records what that is worth — five
concurrent dependabot PRs, each with a different `Cargo.lock`, compiling
"near-cold" and timing out a 30-minute `fmt + clippy + docs`. *Near*-cold, not
cold, is sccache working.

**Rejected alternative.** Deleting the sccache action and `RUSTC_WRAPPER` from the
five compile jobs. It optimises the steady state by making dependency bumps —
already the worst case — worse.

**Date.** 2026-10-22.

## Escalation: sccache on S3

If capping the entry count does not hold the budget, the next move is to take
sccache out of the Actions cache entirely by pointing it at S3
(`SCCACHE_BUCKET` + `SCCACHE_REGION`). That removes ~4.3 GiB and ~2,700 entries
from a 10 GB shared budget in one step, lifts the size cap, and keeps the
dependency-bump resilience that is the reason to keep sccache at all.

The plumbing already exists: `images.yml:95-105` assumes an AWS role through
GitHub OIDC in `us-east-2`, and `infra/terraform` is where the bucket, its
lifecycle expiry and the CI role's policy would go. The cost is a small S3 bill
and one more Terraform-managed role — which is why it is the escalation and not
the first move.
