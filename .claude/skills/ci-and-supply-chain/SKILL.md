---
name: ci-and-supply-chain
description: Explains this repo's CI topology and dependency gates — the `detect`-job pattern that lets a required check skip without blocking a PR, why no required workflow may ever carry a `paths:` filter, the disk-reclaim step the Rust test job needs, the SHA-pinning policy for actions, and the supply-chain gates (cargo-deny advisories/licences/bans, pnpm audit with its reasoned allowlist, and the licence list mirrored between Rust and npm). Use when a CI job fails for reasons unrelated to the diff, when adding or re-wiring a workflow or job, when a dependency bump cannot go green, when an advisory or licence gate fires, or when deciding whether a check should be required. For local gates see rust-conventions; for the frontend test tiers see frontend-testing; for driving a pull request to green and then watching main see land-pr; for the board that tracks the work see project-board; for `infra.yml`'s Terraform tiers, its plan and apply roles and the destroy guard see infra-platform.
---

# CI and the supply chain

Five workflows: `rust`, `frontend`, `infra`, `images`, `drift`. Jobs across the
first three are required status checks, so a change is not mergeable until they
pass. `drift` runs only on a schedule and `workflow_dispatch`, so it can never
be required — a red run there is information about the account disagreeing with
its state file, not about a diff.

Branch protection is repository configuration rather than a file in the tree, so
`gh api repos/:owner/:repo/branches/main/protection` is the only authority on
which; the per-job contexts, timeouts and conditions are generated:
[references/required-checks.md](references/required-checks.md).

**A job's `name:` is its branch-protection context.** Three failure modes
follow, and all brick `main` rather than merely annoy: renaming a required job
silently un-protects it, and adding a required context for a job that does not
yet exist on `main` leaves every PR pending forever. So a split like `entities`
out of `fmt + clippy + docs` goes in one order only: land the job, let it report
green on `main`, *then* add the context.

> **A matrix job's per-leg names cannot be required status checks.**

The third, and the one that is genuinely surprising. While the matrix runs,
`plan` reports one check-run per leg — `plan (core)`, `plan (dev)`, … — which
look like perfectly good contexts. When the job is **skipped by its `if:`**,
GitHub reports a *single* check-run under the literal, uninterpolated name
`plan (${{ matrix.key }})`. The per-leg contexts therefore never report at all
on a PR that skips the tier, and a required context that never reports blocks
the PR forever — the same failure mode as a `paths:` filter, which is what the
`detect` pattern exists to avoid.

Measured, not theorised: the four `plan (<env>)` contexts were made required and
the next non-infra PR went unmergeable with them stuck on "Expected", while
`fmt + validate (credential-free)` was fine throughout because it is not a
matrix job and reports `skipped` under its own name.

The fix is an **aggregator job**: one non-matrix job that `needs:` the matrix
one, inspects `needs.<job>.result` (the aggregate — `success` only if every leg
succeeded), and is required in the legs' place. `infra.yml`'s `terraform plan`
job is the worked example. Two details it gets right: it accepts `skipped`,
because a fork PR legitimately skips the plan tier, and it treats anything it
does not recognise — including an empty result — as a failure.

## The `detect` pattern, and the rule that must not be broken

> **Never add a `paths:` filter to a workflow whose jobs are required checks.**

A workflow that a path filter skips **never reports**, and a required check
that never reports leaves the PR pending forever. That is not a hypothetical —
it is why `frontend.yml` has no `paths:` filter despite only mattering for
frontend changes.

The working pattern instead:

1. A cheap `detect` job computes whether anything relevant changed.
2. Every expensive job gates on it with `if:`.
3. A job skipped by `if:` reports **`skipped`**, which branch protection accepts
   as satisfied.

So the PR stays mergeable at near-zero compute. **`infra.yml` used to carry a
real `paths:` filter**, which was fine only while its jobs were not required;
making the terraform plan a required check meant removing the filter and adding
a `detect` job in the same change, because the two cannot coexist.

`infra.yml`'s detect carries one wrinkle the other two do not: it answers for
`push` as well as `pull_request`, because a merge to `main` triggers an
unattended `terraform apply`. There it **fails safe** — an absent, all-zero or
unreachable before-SHA (force-push, new branch) reports `true` rather than
`false`. A wrongly-skipped plan is cosmetic; a wrongly-skipped apply silently
diverges the account from its configuration until someone notices.

Every `detect` job uses `grep -c`, not `grep -q`. `-q` closes the pipe as soon
as it matches, which can SIGPIPE the `git diff` feeding it under `pipefail`, and
the result is a flaky **false skip** — the worst failure mode available, because
it skips the gate rather than failing it.

> **`skipped` is only trustworthy when the job that decided to skip SUCCEEDED.**

The same false skip is reachable by a second route, and this one is not a bug in
any `detect` job: **if a `detect` job fails, every job gated on it reports
`skipped`, and branch protection accepts `skipped` as satisfied.** A PR can
therefore show all twelve required contexts satisfied with not one of them
having run.

Measured on #473 (2026-11-05): two Rust files changed, all three `detect` jobs
failed in the Actions-minutes outage (cause 0 below), every required context
`skipped`, `mergeStateStatus` **CLEAN**. It was mergeable — by a human or by the
merge bus — with nothing compiled or tested. Of the open PRs at the time it was
the only one in that state, because the others had been pushed to since.

Note this defeats the aggregator gates too: `test` and `DuckLake + extensions
e2e` carry `if: !cancelled() && needs.detect.outputs.code == 'true'`, and with
`detect` failed that output is empty, so the gate's own fail-closed `case`
never executes. The aggregator pattern assumes `detect` succeeds.

Requiring the `detect` contexts in branch protection is **not** the fix — there
are three of them under two names, and a required context that legitimately does
not report blocks every PR forever. What is cheap is checking it at the point of
use: before trusting a PR's green, confirm no check-run whose name starts with
`detect` concluded `failure`:

```
gh api repos/:owner/:repo/commits/<head_sha>/check-runs --paginate \
  --jq '[.check_runs[]|select(.name|startswith("detect"))|select(.conclusion=="failure")]|length'
```

The merge bus does exactly this and refuses such a candidate. Anyone merging by
hand after an outage should too.

`rust.yml`'s detect skips the compile jobs for PRs that touch nothing outside
`.claude/**`, `infra/terraform/**`, `scripts/ci/tf-*` and the two terraform
workflows (`infra.yml`, `drift.yml`). That set and no more — `docs/` changes
still run the gate, because `entities` verifies `docs/erd.md` and `lint`
verifies the generated skill references.

Each entry is there because nothing in the Rust gate reads it: no crate or
manifest references `infra/`, and `make check` touches it nowhere. The one real
coupling is that `skills-check.sh` derives generated skill references *from*
`infra/terraform/**` — `aws_*` resource counts, IAM users, Pod Identity
associations, pinned provider versions — so a terraform-only PR can make a
reference stale. That is covered by the ungated `skills` job rather than by the
compile jobs, which is precisely why that job exists separately.

The workflows are named **file by file**, not as `^\.github/workflows/`, and the
distinction is load-bearing: `rust.yml`, `frontend.yml` and `images.yml` must
keep triggering the gates they define, or a change to a gate would stop
reporting on exactly the PR most likely to break it. The terraform workflows
define no Rust job and cannot affect the workspace build, while an infra-only PR
was paying the full ~40-minute backend gate — `test` plus the DuckLake e2e — to
establish that.

Enumerating, rather than exempting the directory and listing exceptions, is the
**fail-safe** direction: a workflow added later runs the gate until someone
argues it should not, instead of skipping it until someone notices.

Note the asymmetry between the three implementations, because it decides what a
new entry does. `rust.yml`'s detect is a **denylist** (count paths *not* under
these trees), so adding a tree there makes the Rust gate stop seeing it — only
safe for a tree nothing in `make check` reads. `frontend.yml`'s and
`infra.yml`'s are **allowlists** (count paths *under* `frontend/`, under
`infra/terraform/`), so a tree they do not name is already skipped, and the
risk runs the other way: a tree that *should* gate them has to be added by
hand. A terraform PR skips the frontend gate for free and never reaches
`images.yml`, which runs on `push` only.

## Merge queue and content-addressed skips

`rust`, `frontend` and `infra` all trigger on `merge_group`, so `main` is
protected by the merge queue instead of "require branches to be up to date": a
PR is green against the base it was opened on, and the queue re-tests the exact
tree that will land. **Adding a required job to a workflow without
`merge_group:` stalls every queued PR**, the same failure as a `paths:` filter.
In the queue, each `detect` diffs `merge_group.base_sha...head_sha` with its
usual filter; `rust.yml`'s `bench-compile`/`scale-tests` exclude the event, and
`infra.yml`'s `plan` skips there (accepted by `plan-gate`).

On a PR, `rust` and `frontend` `detect` also compute a **pass key**: a hash of
the tested tree over every path the gate reads (Rust: the whole tree minus the
filter's own exempt regex; frontend: the filter's own allowlist) plus the ISO
week. A `ci-pass-*` cache hit means that content already went green on this PR,
so the gate skips — the usual case being a rebase over a main that moved only
through paths the gate ignores. `record-pass` writes the marker after every
required job succeeds. Markers are PR-ref-scoped (a PR can only skip on its own
result), are never consulted on `push` or `merge_group`, and expire weekly to
bound floating inputs (stable rustc, advisory DBs). The `ci:full` label forces
a full run. If the key and the filter ever disagree about what a gate reads,
fix the filter's regex — the key reuses it verbatim.

## Why a job fails when your diff is innocent

Four recurring causes, worth checking before debugging the change.

**0. The account ran out of Actions minutes, and it looks nothing like it.**
Check this one first, because it is the only one that is not about your branch
at all and the only one no amount of reading the diff will explain. The
signature:

- every job fails within **seconds**, including trivial ones like
  `detect changes`
- the job has **`steps: []`** — nothing executed
- its log is a `BlobNotFound` 404; there is no log, because there was no runner
- **every workflow on every branch** fails at once, `main` included
- githubstatus.com reads **operational** for Actions throughout

Read it from **execution, not conclusion**: a starved run and a legitimately
failing one are both `failure`, but only one of them ran a step.

```
gh api repos/:owner/:repo/actions/runs/<run_id>/jobs --jq '[.jobs[].steps | length] | add'
```

Zero across every job means no runner was allocated. Measured on this repo:
starved runs summed to **0**, real ones to **5, 5 and 202**. Nothing in CI can
go green until the spending limit is raised or the monthly allowance resets —
and that is an account setting, not a repository one. Observed 2026-11-03,
sustained for hours while `main` went red for the same reason.

The merge bus checks for this before it builds anything, and distinguishes it
from a bad batch afterwards; see below.

**1. Supply-chain gates are time-dependent.** `cargo-deny` and `pnpm audit`
fail on advisories and yanked releases published **since the last green run**.
Nothing in your diff caused it.

Read the **job log**, not the diff — the log names the advisory. Fix it in its
own small PR rather than entangling it with a feature, so the feature's review
stays about the feature.

**2. Disk exhaustion in the Rust `test` and e2e jobs.** A full `rust/target` is
tens of gigabytes, and those jobs carry a **disk-reclaim step** which deletes
preinstalled toolchains to recover space.

The symptom is misleading: **`linking with cc failed` usually means the disk is
full**, not that anything is wrong with the link. If you see it, check whether
the reclaim step is still present and sufficient.

**3. A cold rebuild after a dependency bump.** The compile jobs share an sccache
cache and link with mold, but a `duckdb` bump invalidates the cache and forces a
full rebuild — which is why the lint and DuckLake e2e jobs carry longer
timeouts than their steady-state runtime needs.

A bump is not the only way to land cold, though — it used to be the lesser one.
**The Actions cache is a shared, capped resource**: GitHub allows 10 GB per
repository and evicts LRU past it, and while sccache lived on the GHA backend
its one-entry-per-compilation-object count was what evicted the rust-cache
entries that make a run warm. That budget was the difference between a
14-minute and a 42-minute gate, and it is why sccache moved to **S3**
(`SCCACHE_BUCKET` in rust.yml, bucket + OIDC roles in
`infra/terraform/envs/core/sccache.tf`, shipped 2026-10-29) — no shared budget,
no eviction class. The Actions cache still holds rust-cache and buildkit layers,
so the prune job survives. Where the time actually goes, what the caches cost,
and the decision log for changes against them:
[references/build-cost.md](references/build-cost.md).

## Pinned actions

Every `uses:` is pinned to a **commit SHA** with a `# vTAG` comment. A tag is
mutable; a SHA is not. The comment is what makes the pin reviewable, so keep
both halves when bumping.

One consequence: the toolchain is installed with `rustup` rather than an action,
because the action selects its channel through its ref and so cannot also carry
a SHA pin.

`bash scripts/ci/actionlint.sh` is the check, and the `workflow pins (S9)` job
in `rust.yml` runs it. **It is ungated, deliberately** — `detect` exempts
`.claude/**`, `infra.yml`, `drift.yml` and every `*.lock.yml`, so a gated pin
check would skip on exactly the PRs most likely to introduce an unpinned
action. It is **not yet a required check**; promote it once it has reported
green on `main` and been observed failing, in that order.

Two things it gets right, both learned the hard way:

- **It is pure bash, with no dependencies.** It used to shell out to
  `uv run python scripts/actionlint.py` and import `er.versions` for a pinned
  `actionlint-py` wheel. The Python/dbt removal took that script, the root
  `pyproject.toml` and `uv.lock` with it, so the checker could not run at all —
  and because nothing invoked it, the breakage was invisible while this section
  went on calling S9 enforced. Found 2026-11-03. A gate with no dependencies
  cannot be deleted by a dependency cleanup.
- **actionlint proper skips the generated workflows, and the pin check does
  not.** actionlint 1.7.7 reports 40+ findings across the five `gh aw`-compiled
  files and not one is a defect: `runs-on: ubuntu-slim`,
  `concurrency.queue: max`, `job.check_run_id`, `job.workflow_ref` and
  `permissions.vulnerability-alerts` are all real GitHub features newer than the
  linter's tables. Linting them would leave this gate permanently red, which
  teaches people to ignore it. Generated-ness is read from each file's own
  header marker rather than a filename pattern, so the next gh-aw workflow is
  excluded automatically — but the **S9 pin check still covers them**, because
  `gh aw compile` pinning for you is not the same as anyone checking.

Zero `uses:` matches across the whole tree is a **failure**, not a clean run:
if the pattern ever stops matching, every workflow trivially passes and the
policy lapses silently, which is the state it was found in.

## What is deliberately not required

`semgrep` runs but is **not** a required check: its registry packs need network,
and a flaky required check is worse than none. Read its output anyway — and
know the trap, which is that **semgrep reports zero findings when a ruleset
fails to parse**, indistinguishable from clean.

The nightly and main-only jobs (coverage, benchmark compile, scale goldens) are
not required either, by construction.

## The merge bus

`main`'s protection has **`strict: false`** — "require branches up to date
before merging" is off — so a PR may merge on a green it earned against a base
that has since moved. With five or six PRs open, four each green against an
older `main` can all merge and leave `main` red, because no run ever examined
the combination. `merge-bus.yml` is that missing run: it batches the PRs
carrying the `merge-bus` label, opens a PR for the combination so the ordinary
gate judges it, and squash-merges the members only if that goes green.
Dispatch-only, `dry_run: true` by default, report-and-stop on red.

GitHub's **merge queue is this feature and cannot be enabled here** — it is
organization-only, private repositories needing Enterprise Cloud, and this one
is private and user-owned.

Four things bite, and each cost a live run or a false green to learn: it needs
**`actions: read`** (an unnamed scope is `none`); it can **never carry a PR
touching `.github/workflows/`** (`workflows` is not a grantable `GITHUB_TOKEN`
scope); it refuses a PR whose **`detect` jobs failed**, because that green
verified nothing; and `merge bus` **must never become a required status check**,
since a dispatch-only job can never report on a PR.

The full design, every refusal reason, the two landing invariants and the
simulator: [references/merge-bus.md](references/merge-bus.md). Run
`bash scripts/ci/merge-bus-test.sh` when you touch it.
## Agentic workflows

Four `gh-aw` workflows and the `agent-task.yml` dispatcher are **merged and
disabled**: `engine: copilot` needs a real `COPILOT_GITHUB_TOKEN`, the repo has
no secrets, and all four therefore died at `activation` on every trigger. They
fail *closed* — the agent, detection and `safe_outputs` steps skip and nothing
is posted — so a disabled run is not a silent one.

Re-enabling them, the safe-output boundary, the `sccache` env that must match
`rust.yml` byte-for-byte, and the disk-reclaim step that must NOT be copied are
all in [`references/agentic-workflows.md`](references/agentic-workflows.md).

## The supply-chain gates

| Gate | Enforces |
|---|---|
| `cargo-deny check advisories licenses bans sources` | Rust advisories, the licence allowlist, duplicate-version warnings, and that crates come only from the official registry or an allowlisted git source |
| `pnpm audit:check` | **high and critical** npm advisories only; moderate and below are reported |
| `pnpm licenses:check` | the npm licence **allowlist** |

Three properties that make these usable rather than noise:

- **Every `deny.toml` ignore carries a prose justification** in the file. An
  ignore without a reason is not acceptable, and each is meant to be removed
  when upstream moves.
- **`audit-allow.json` entries require a `reason` field** — the script errors
  without one — and **stale entries that match no current advisory are
  reported**, so the allowlist cannot silently accumulate.
- **`check-licenses.mjs` is an allowlist, not a denylist.** An SPDX expression
  it cannot parse fails rather than passing. It copies `deny.toml`'s allow list
  **verbatim** as `DENY_TOML_ALLOW` and unions npm-specific additions, each with
  its own justification — so the two ecosystems cannot drift apart silently.
  **Change both together.**

The pins that cannot be bumped alone, and the full dependency-bump workflow:
[references/dependency-bumps.md](references/dependency-bumps.md).

## Images

`images.yml` builds on pushes to the default branch, assuming the ECR push role
through GitHub OIDC, and tags by **commit SHA** — there is no `latest`. That is
why `make dev-up` requires an explicit `TAG`.

## Common workflows

**A required check is pending forever.** Someone added a `paths:` filter to a
required workflow, or renamed a job (its display name *is* the context branch
protection matches). Check the name first.

**cargo-deny went red overnight.** A new advisory. Read the job log, then
either upgrade or add an ignore **with a justification**, in its own PR.

**A dependabot PR cannot go green.** Check first whether the dependency is
pinned to another crate's major — `arrow` to `duckdb`'s, `datafusion` to
`ff-analysis`'s. Those three are now excluded from major updates in
`.github/dependabot.yml` (each with the compiler error that proved it), so the
bot should not propose them at all; if one appears anyway, close it. The
unblocker is upstream, never a fix in the PR.

**Adding a gate.** Decide whether it should be required, and make it report on
every PR if so (the `detect` + `if:` pattern). Then **observe it failing**
before trusting it, and give it a rename-detection exit so it cannot pass by
examining nothing.

**A merge-bus batch went red.** The batch is not bisected, so the comment on
each member says only that the combination failed — it does not accuse that PR.
Read the bus PR's own checks to see which context failed, and note that a
combination can fail when every member passes alone, which is the whole reason
the bus exists. Re-run the bus with `max_batch=1` to attribute it.
