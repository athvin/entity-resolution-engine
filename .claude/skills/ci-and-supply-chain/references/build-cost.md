# Build cost: where Rust CI time goes

Every timeout in `rust.yml` carries the measurement that justifies it, which is
the right place for a *timeout*. What that leaves out is the cross-job picture —
which job is the critical path, how far cold diverges from warm, and why cold
happens at all. This file is that picture, and the decision log for changes made
against it.

## Contents

- The measured baseline (2026-10-22)
- The first measurement after the change
- Where it ended up
- Why cold happens: the Actions cache budget
- The levers, and which were taken
- Rejected: headstart
- Blocked: more cores per job
- Rejected: removing sccache
- Next step: sccache on S3

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

## The first measurement after the change

Run `39775982197`, the cache + profile change on a PR. **Read this run with its
caveat first: the baseline's rust-cache restore step took 0m01 — a true miss —
and this one took 0m28, an actual restore.** So it is not a clean A/B of the
profile change; it is "profile change, and a cache entry existed". One data point
cannot separate those, and nothing below should be read as attributing the
improvement to one of them.

| Job | baseline (miss) | run 39775982197 |
|---|---|---|
| fmt + clippy + docs | 42m27 | **19m57** |
| DuckLake + extensions e2e | 27m55 | 23m10 |
| test | 14m22 | 12m52 |
| **workflow wall clock** | **42m38** | **23m18** |

**The step breakdown is the useful part, because it overturned a design
decision:**

| Step | baseline | run 39775982197 |
|---|---|---|
| `cargo clippy` | 10m32 | 9m22 |
| `er-entity verify` | 10m39 | 8m16 |
| `cargo doc` | 9m20 | **0m39** |
| OpenAPI gate (advisory) | 10m34 | **0m42** |

**What that means: the cost is the dependency tree, paid once per kind of build,
and the workspace's own 28 crates are cheap.** `cargo clippy` builds the tree's
metadata; the first `cargo run` pays its codegen; everything after that in the
same job is nearly free. The earlier reasoning here — that clippy's `.rmeta`
cannot help a `cargo run`, and that `er-entity-cli` (datafusion 52 / arrow 57)
and `er-server` (duckdb / arrow 58 / aws) share too little to matter — is
**contradicted by that 0m42**, and was the justification for giving the OpenAPI
gate its own job. It does not have one; it is a step on `entities`, where it
costs 0m42 instead of re-paying a tree build.

So only two of the four steps earned their own job, and `fmt + clippy + docs`
became two, not three: it keeps fmt, clippy, rustdoc and the skills-reference
check, and `entities` takes `er-entity verify` plus the advisory gate (PR-only —
on a push to main it has no base to diff and exits having examined nothing).

**`DuckLake + extensions e2e` was left whole for the same reason, inverted.** It
is now the critical path, so splitting it is the obvious next move, and the
numbers say no: its 8m17 extension step ran *after* the tree was already built,
so unlike rustdoc's 0m39 those 8m17 are its own test-target compile, link and
run. Standalone it would pay a tree build on top — plausibly ~18m against the
remaining 13m15, making the wall clock worse than the 23m10 it was meant to fix.
What would settle it is the tree cost of a `-p dedupe-core --test extensions`
build alone, which no run has isolated.

**sccache hit rates, visible for the first time** (this run, where the profile
change invalidated every fingerprint, so these are a floor and not a steady
state):

| Job | requests | hits | misses | hit rate | `target` |
|---|---|---|---|---|---|
| fmt + clippy + docs | 1724 | 273 | 1172 | 16% | 13G |
| test | 945 | 385 | 333 | 41% | 24G |
| DuckLake + extensions e2e | 1158 | 314 | 691 | 27% | 20G |

Note the C/C++ hits (104 / 171 / 223) — sccache is caching part of the bundled
DuckDB compile, not just Rust. A steady-state reading on an unchanged lockfile is
the number that decides the S3 question below; this is not it.

### The profile change was invisible to rust-cache

Found on main run `39779467231`, the merge of the profile change. The `lint` job
logged:

```
Cache hit for: v0-rust-rust-Linux-x64-8556beb2-9ec4ef33
...
Cache up-to-date.
```

**An exact key hit, so rust-cache skipped the save.** `rust-cache` keys on
`Cargo.lock`, the rustc version and `.cargo/config.toml` — **not** on
`Cargo.toml`, where `[profile.*]` lives. So the pre-profile entry became
immortal: every run restored ~1.3 GB of full-debug-info dependency artifacts
whose fingerprints cargo then rejected and rebuilt, and the smaller
post-profile `target` was never saved. Two useless entries held ~3 GB of a 10 GB
budget that was already evicting.

The fix is two inputs on every rust-cache use, which must stay identical across
jobs or they stop sharing an entry:

- `prefix-key: "v1-rust"` — makes the `v0-*` entries unreachable (restore-keys
  are prefixed too) so they age out instead of being downloaded for nothing. One
  fully cold run follows, by design.
- `key: ${{ hashFiles('rust/Cargo.toml') }}` — the durable half. A future profile
  or lint edit lands on a new key and therefore *saves*, while restore-keys still
  prefix-match the previous entry for a partial restore.

**The general lesson, worth more than the fix:** a cache whose key does not cover
an input that changes build output does not go stale loudly — it goes stale
*silently*, keeps reporting hits, and the cost shows up as compile time nobody
attributes to it. `Cache up-to-date` next to an entry whose timestamp predates a
build-affecting change is the signature.

### The prune has to delete concurrently

The first version of `cache-prune` deleted sequentially, and that does not fit in
any sane timeout. One `DELETE` is a separate HTTPS round trip (~150 ms), and the
backlog it was first run against was **5,595 entries / 5,846 MiB**, i.e. ~3,400
deletions. Run against the live cache it was still going after **ten minutes**,
against a 15-minute job timeout — so the nightly job would have died partway and
reported failure while leaving the budget blown.

Fixed by choosing the victims in one `awk` pass and deleting them through
`xargs -P 8`: **1,586 deletions in 2m00** measured, so the 4,000-deletion cap
lands around five minutes. The timeout went to 30 minutes, because listing ~5,600
entries over 56 paginated requests is itself minutes before any deleting starts.

Two details that are load-bearing rather than defensive:

- **`|| true` on each delete.** A concurrent run may already have evicted that
  entry. Without it `xargs` exits 123 and, under `set -e`, fails a job whose only
  purpose is hygiene.
- **`MAX_DELETES` bounds one run** so the job cannot become unbounded work, and
  it *reports* when it stops short. A prune that silently gives up looks exactly
  like a prune that finished.

First measured result, after relieving the backlog by hand: **10,999,396,329 →
7,029,161,413 bytes (6.54 GiB) across 4,485 entries** — under the ceiling with
~3.4 GB of headroom, which is where the eviction stops.

### Where it ended up

Run `39783867347` — profile change, single cache writer, and the `entities`
split, on a warm cache:

| Job | baseline | now |
|---|---|---|
| fmt + clippy + docs | 42m27 | **3m33** |
| entities | (inside the above) | 4m09 |
| test | 14m22 | 7m54 |
| DuckLake + extensions e2e | 27m55 | 7m04 |
| cargo-deny | 30s | 0m25 |
| **workflow wall clock** | **42m38** | **8m03** |

The split did what it was supposed to: 3m33 and 4m09 **in parallel**, where the
same two gates were ~7m42 serially. The critical path is now `test` at 7m54,
which is a `cargo test --workspace` and a 2m03 disk-reclaim step — a different
problem from the one this file opened with.

### sccache is the primary cache, and that was backwards here

**The same run logged `No cache found.` for rust-cache in both compile jobs.** A
total miss, no restore at all — and it still finished in 8m03:

| Job | requests | hits | misses | hit rate | C/C++ hit rate |
|---|---|---|---|---|---|
| fmt + clippy + docs | 1068 | 779 | 27 | **96.65%** | 98.32% |
| entities | 1113 | 992 | 25 | **97.54%** | 95.66% |

clippy ran in 2m02 with no rust-cache entry whatsoever. So for this workspace
**sccache is the cache that matters**, and it covers the bundled DuckDB C++
compile as well as the Rust units.

That inverts the framing at the top of this file. The eviction problem was real —
sccache's entry count was starving rust-cache — but the conclusion drawn from it
was half wrong. Protecting the rust-cache entry with a single-writer rule was
guarding the *cheaper* of the two caches, and the nightly prune's first budget
(2048 MiB) capped the expensive one. Each pruned entry is a future compile.

Two consequences, both applied:

- `SCCACHE_BUDGET_MIB` is now **5120**, framed as "what is left after the caches
  that cannot share" rather than as sccache's slice.
- **The S3 move below stops being a contingency and becomes the obvious next
  step.** Capping the primary cache to protect a 10 GB shared budget is the wrong
  trade; taking it out of that budget removes the trade entirely.

A second run settles it. Run `39786067377` carried the `prefix-key: v1-rust`
bump, so rust-cache was guaranteed to miss — `No cache found.` in every job:

| Job | requests | hits | misses | hit rate | job time |
|---|---|---|---|---|---|
| fmt + clippy + docs | 1068 | 801 | 5 | **99.38%** | 1m53 |
| entities | 1113 | 1011 | 6 | **99.41%** | 4m19 |
| test | 945 | 715 | 3 | **99.58%** | 8m36 |

Wall clock **8m45** with no rust-cache entry in existence, against 8m03 with one
that also missed. `fmt + clippy + docs` in 1m53 from nothing but sccache.

**So the open question is now the other way round: what is rust-cache for?** It
holds ~1.3 GB of a 10 GB budget, costs a restore and a save, and across two runs
contributed nothing measurable because sccache answered 99% of compilations. The
next experiment is a PR that removes it from the Rust jobs and compares; that
would free the budget outright and make the S3 move unnecessary rather than
merely attractive. Not done here — it is a deliberate change deserving its own
measurement, and two runs that never restored an entry cannot prove what
restoring one would have been worth.

Read the 42m38 → 8m03 as the span between a cache miss on the old shape and a
warm sccache on the new one, not as a single intervention. Three things changed
at once, and the step-level numbers support each contributing.

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
| `save-if` so one job writes each key | yes | stops five jobs racing the same key and PR branches minting entries — though see the sccache finding: this protects the cache that turned out not to matter |
| Drop `coverage`'s cache save | **no** | frees 1.8 GiB but makes a 22m10-warm job permanently cold against a 60-min timeout; the profile change shrinks the entry instead |
| Prune sccache to a budget, nightly | yes, but raised to 5120 MiB | caps the entry count that does the evicting. Must delete concurrently. The first budget (2048) throttled the cache doing 97% of the work — see the sccache finding |
| `sccache --show-stats` in each job | yes | the hit rate was never observable; it is the input to every later decision here |
| Key rust-cache on `Cargo.toml` too | yes | it does not by default, so the `[profile.*]` change hit an exact key, logged `Cache up-to-date` and never saved — leaving an immortal pre-profile entry |
| Split the serial `fmt + clippy + docs` | yes | 2026-10-22: `entities` split out and made required. Only the two tree-paying steps earned a job — see the measurement, which overturned splitting the other two |
| Split `DuckLake + extensions e2e` | **no** | tried, then reverted on the numbers: its 8m17 step is its own work, not a tree it inherited, so a separate job would pay a tree build on top and come out slower. Now the critical path, so this is the open question |
| cargo-nextest | no | build time dominates execution time here; revisit after the above |
| `.cargo/config.toml` for local mold/sccache | no | helps laptops, not CI; worth doing, not yet scoped |
| cargo-chef + cache mounts in `rust/Dockerfile` | no | `COPY . .` + one `cargo build --release` means any source change rebuilds DuckDB in release mode; real, separate |
| More cores per job | **blocked** | not reachable by editing `runs-on:` on a user-owned repo — see below |
| Drop rust-cache from the Rust jobs | **open — next experiment** | two runs completed in 8m03 and 8m45 with `No cache found.` and sccache at 99%; it holds ~1.3 GB and contributed nothing measurable |
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

## Next step: sccache on S3

**Promoted from contingency to the recommended next move** by the 97% hit rate
above: sccache is the primary cache, so capping it to protect a shared 10 GB
budget is the wrong trade. Take it out of the Actions cache entirely by pointing
it at S3
(`SCCACHE_BUCKET` + `SCCACHE_REGION`). That removes ~4.3 GiB and ~2,700 entries
from a 10 GB shared budget in one step, lifts the size cap, and keeps the
dependency-bump resilience that is the reason to keep sccache at all.

The plumbing already exists: `images.yml:95-105` assumes an AWS role through
GitHub OIDC in `us-east-2`, and `infra/terraform` is where the bucket, its
lifecycle expiry and the CI role's policy would go. The cost is a small S3 bill
and one more Terraform-managed role — which is why it is the escalation and not
the first move.
