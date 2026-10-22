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
- Blocked: more cores per job
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

That job is now three (2026-10-22): `fmt + clippy + docs` keeps fmt, clippy,
rustdoc and the skills-reference check; `entities` takes `er-entity verify`; and
`api-gate` takes the advisory OpenAPI diff, PR-only, since on a push to main it
has no base to diff and exits having examined nothing. Cold critical path becomes
the longest of them rather than the sum.

**Why they could not simply share one build.** clippy emits `.rmeta` check
artifacts and `cargo run` needs linkable rlibs, so the original job paid for a
check build *and* a debug build, and the second was no cheaper for the first
having run — which is why four steps each cost ~10 minutes rather than one
costing 10 and three costing little. The two `cargo run` steps do not share much
with each other either: `er-entity-cli` pulls er-sql-check's datafusion 52 /
arrow 57 universe and `er-server` pulls duckdb / arrow 58 / aws, and
`er-sql-check`'s manifest records that the two arrow majors never unify. So
separate jobs duplicate less than the crate count suggests.

`DuckLake + extensions e2e` had the same shape and got the same treatment: its
16m36 four-crate test and 9m47 rapidfuzz extension test were serial, so the job
cost their sum. `extensions (rapidfuzz)` is now its own job, which also drops the
Postgres service it never used — that existed for the er-server control-plane
queue gate in the other half.

Elsewhere: `test` spends 2m33 in the disk-reclaim step before compiling, and
10m47 in `cargo test --workspace`. `coverage` is 22m10 of `llvm-cov` alone,
nightly-only, and deliberately opts out of sccache and mold (`rust.yml:358-360`).

**Expected critical path after both splits**, holding the cold step costs
constant: `max(clippy 10m32 + rustdoc 9m20, entities 10m39, api-gate 10m34,
ducklake 16m36, extensions 9m47, test 14m22)` ≈ **20m**, against 42m38 measured.
Re-measure rather than trusting that arithmetic — it assumes six jobs restoring
one cache entry in parallel costs each of them what it cost one job serially,
which is exactly the kind of assumption the `sccache --show-stats` output exists
to check.

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
| Split the serial `fmt + clippy + docs` | yes | four independent ~10m steps; the advisory one was pure critical-path tax. Done 2026-10-22: `entities` + `api-gate` split out, `entities` required |
| cargo-nextest | no | build time dominates execution time here; revisit after the above |
| `.cargo/config.toml` for local mold/sccache | no | helps laptops, not CI; worth doing, not yet scoped |
| cargo-chef + cache mounts in `rust/Dockerfile` | no | `COPY . .` + one `cargo build --release` means any source change rebuilds DuckDB in release mode; real, separate |
| More cores per job | **blocked** | not reachable by editing `runs-on:` on a user-owned repo — see below |
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

## Blocked: more cores per job

**Decision.** More cores is wanted (operator: "having fast CI is really
important") but is **not reachable by editing `runs-on:`**, so the parallelism
above was taken first. Any core increase needs an account-level decision.

**Context — what this repo can actually run.** The repository is **public and
owned by a user account**, not an organisation. That fixes two things:

- `ubuntu-latest` is 4 vCPU / 16 GB, and **its minutes are free**, because
  standard GitHub-hosted runners are free for public repositories. The current
  bill for all of this CI is zero.
- **GitHub "larger runners" are not available.** They are a Team / Enterprise
  Cloud feature, provisioned as a runner group in *organisation* settings, and
  they are **metered even on public repositories** (~$0.012/min for 4-core
  Linux, scaling with size). A user-owned repo has no org settings page to
  create one in. `gh api repos/.../actions/runners` also confirms no self-hosted
  runner is registered.

So the failure mode to avoid is writing a label like `ubuntu-latest-8-core` that
does not resolve: the job then queues forever, and on a **required** check that
blocks every PR — the same class of mistake as a `paths:` filter on a required
workflow.

**The three real paths, none of them a one-line edit.**

1. **A third-party runner provider** (Blacksmith, Depot, BuildJet, Namespace,
   WarpBuild). Install a GitHub App, then change `runs-on:` per job. Several have
   free or discounted open-source tiers, and — relevant here — several ship their
   own cache backend with a far higher ceiling than 10 GB, which would retire the
   eviction problem this file is mostly about rather than just outrunning it.
   Lowest friction, and the only option that addresses both axes at once.
2. **An organisation on the Team plan**, which unlocks GitHub larger runners.
   Transfer the repo, create a runner group, then use its label. Keeps everything
   first-party; costs the plan plus metered larger-runner minutes, and gives up
   the free-for-public-repos standard-runner minutes for any job moved over.
3. **Self-hosted runners** on the existing AWS/EKS footprint. **Weigh this
   carefully on a public repo:** a pull request from a fork can execute arbitrary
   code on a self-hosted runner, which is why GitHub advises against self-hosted
   runners on public repositories. It is mitigable (fork PRs gated behind manual
   approval) but it is a real attack surface on a repo that holds a `security`
   skill and a pentest programme, and it carries infrastructure cost and
   maintenance that the other two do not.

**What was done instead, and why it was done first.** Parallelism, which is free:
`fmt + clippy + docs` and `DuckLake + extensions e2e` were each running
independent steps serially, so they cost the sum rather than the longest. That
takes the cold critical path from 42m38 to roughly 20m without an account
decision. A core increase multiplies whatever remains after that — and buying
cores before fixing a blown cache budget and a serial job graph would have hidden
both faults while paying for them on every run.

**Deferred, deliberately** (operator, 2026-10-22): take the free parallelism
first, re-measure, then revisit cores with numbers. The expected ~20m critical
path may be enough on its own, and a core increase multiplies whatever is left
rather than being wasted by it.

**The scope is already decided, so a revisit need not re-litigate it:** the
**longest required** jobs only — `DuckLake + extensions e2e`,
`fmt + clippy + docs`, and `test`. Not the cheap jobs, not `entities` /
`api-gate` / `extensions` (already ~10m and off the critical path), and not the
nightly ones. Pay only where wall clock gates a merge; everything else keeps the
free standard runner.

**Still open.** Which of the three paths. Note that path 1 or 2 is also
precondition 1 for a [headstart](#rejected-headstart) revisit.

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
