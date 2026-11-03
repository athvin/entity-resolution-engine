---
name: ci-and-supply-chain
description: Explains this repo's CI topology and dependency gates — the `detect`-job pattern that lets a required check skip without blocking a PR, why no required workflow may ever carry a `paths:` filter, the disk-reclaim step the Rust test job needs, the SHA-pinning policy for actions, and the supply-chain gates (cargo-deny advisories/licences/bans, pnpm audit with its reasoned allowlist, and the licence list mirrored between Rust and npm). Use when a CI job fails for reasons unrelated to the diff, when adding or re-wiring a workflow or job, when a dependency bump cannot go green, when an advisory or licence gate fires, or when deciding whether a check should be required. For local gates see rust-conventions; for the frontend test tiers see frontend-testing.
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
that has since moved. With five or six PRs open at once, four PRs each green
against an older `main` can all merge and leave `main` red, and no run ever
examined the combination. `merge-bus.yml` is that missing run: it batches the
PRs that are ready, opens a PR for the combination so the ordinary gate judges
it, and merges the members only if that goes green.

GitHub's **merge queue is this feature and cannot be enabled here** — it is
offered only for organization-owned repositories, public on any plan or private
on Enterprise Cloud, and this one is private and user-owned. Probed, not
assumed: `rulesets` is `[]` and `mergeQueue(branch:"main")` is `null`. The
self-hosted alternatives are worse rather than better: bors-ng is archived, and
rust-lang/bors and Prow's `tide` are long-running GitHub Apps needing a hosted
service, a webhook endpoint and an App key — more standing credential than a
workflow file, and each restates the required checks in bot config where they
can drift from protection silently.

Four properties worth knowing before changing it:

- **It opens a pull request, and that is why it cost no edits elsewhere.** The
  three required workflows trigger on bare `pull_request`, so a batch branch
  earns every context with correct `detect` behaviour. Wiring the native queue
  would have been *more* work: `merge_group` is not a `pull_request` event, so
  all three `detect` jobs would fall through to their fail-safe default and
  every merge-group build would run every gate at full cost.
- **Eligibility asks GitHub rather than restating the required set.** With the
  review requirement at zero, `mergeStateStatus == CLEAN` *is* "every required
  check is satisfied", so the bus enumerates no contexts and cannot drift from
  protection. `DIRTY` is rejected **by name**, because a conflicted PR runs zero
  workflows and reports no checks — any predicate phrased as "nothing is
  failing" reads that as green. Note `gh pr list` returns `UNKNOWN` and does not
  trigger the computation; only `gh pr view` does.
- **It tells a bad batch apart from a starved account.** Cause 0 above presents
  as `failure` on every required context, which the bus would otherwise report
  as "the batch went red" — sending someone to debug a diff that was never
  compiled. So it refuses to build a batch when none of the last five completed
  runs executed a step, and if capacity runs out mid-validation it reports
  `starved` instead of red: no `bus-failed` label, and no comment on the
  members, because during an outage every PR in the repository is equally stuck.
- **It stops on red and does not bisect.** A failed batch merges nothing,
  comments the failing contexts on the bus PR and every member, and leaves the
  bus PR open because its branch is the only place the failing combination
  exists. A *conflict* is ejected instead of stopping the batch — that is found
  before any CI is spent and names the culprit exactly.
- **Two invariants make the landing sound.** `main` must still be at the SHA the
  batch was cut from, or the green no longer describes what merging would
  produce; and after the squashes, `main`'s **tree** must equal the tree the
  gate examined. The second exists because `state: MERGED` is not evidence —
  a stacked PR whose base branch is deleted is marked merged, with a real
  `mergeCommit`, while its code never reaches `main`. For the same reason the
  bus retargets stacked children onto `main` *before* merging their parent,
  which is the only time `gh pr edit --base` still works.

`merge bus` **must never become a required status check**: it runs on
`workflow_dispatch` only, so it can never report on a PR, and a required
context that never reports blocks every PR forever. It is also the first
hand-written workflow here with `contents: write` and `pull-requests: write`
— it pushes a branch and merges PRs — and it adds no repository secret to get
them. It is deliberately **not** an admin, which tightens rather than loosens
things: every merge it makes must satisfy the required checks, where a human
running `gh pr merge --admin` bypasses all of them.

## Agentic workflows

`.github/workflows/*.md` are [`gh aw`](https://github.github.io/gh-aw/) agentic
workflows. **Edit the `.md`; `gh aw compile` emits the `.lock.yml` beside it.**
A `.lock.yml` is compiler output — treat it exactly like any other generated
file here and never hand-edit it. `references/required-checks.md` tabulates
which ones exist, what fires each, and what each is permitted to do.

**None of them may ever be a required check.** A model run is non-deterministic
and rate-limited, so requiring one would gate every merge on a retry. That is
also the one thing they have that `rust.yml` and `frontend.yml` do not: because
they are not required, they *are* allowed `paths:` and `if:` filters. The rule
in CLAUDE.md is about required workflows specifically.

They also hold almost all of the repo's GitHub write scopes.
`pull-requests: write` and `issues: write` appear nowhere else, and
`contents: write` appears in one other place: the `agent` job of
`agent-task.yml` (below). The other hand-written workflows are `contents: read`
plus `id-token: write` for OIDC.

**The model's own job holds none of them.** gh-aw compiles each workflow into
five or six jobs, and the `agent` job that runs the model gets only read scopes;
`contents: write` and friends live in the separate `safe_outputs` and
`conclusion` jobs that run after it exits. gh-aw's **safe outputs** are declared,
capped operations (`add-comment`, `push-to-pull-request-branch`, …) which those
jobs validate — so the model's output is data they check rather than commands
they obey. Worth verifying rather than trusting, and easy to: extract the
per-job `permissions:` blocks from a `.lock.yml`.

One exception, stated because "the agent never writes" is otherwise an
over-claim: the two agents that compile Rust carry `id-token: write` *on the
agent job*, because `shared/rust-gate.md` exchanges it for the sccache role.
That mints an AWS credential, not a GitHub one, and the role is
`er-ci-sccache-read` — `s3:GetObject` and `s3:ListBucket` on one bucket, with no
`DeleteObject` in the policy at all. Read access to compiler output is the whole
blast radius, and it is read, so an agent cannot plant object code a later
`main` build consumes.

**The zero-repository-secrets invariant does not survive them, and the claim
that it did was false.** This section used to say `engine: copilot` draws on
`copilot-requests: write` against the job token, so no secret was needed. On
first real execution (2026-11-03, the run after #359 landed) all four agents
failed at `activation`:

```
Error: None of the following secrets are set: COPILOT_GITHUB_TOKEN
COPILOT_GITHUB_TOKEN is required by GitHub Copilot CLI.
```

The generated `activation` job runs `validate_multi_secret.sh
COPILOT_GITHUB_TOKEN` and hard-fails without it. **There is no zero-secret path
to running these at all** — `copilot` needs `COPILOT_GITHUB_TOKEN`, `claude`
needs `ANTHROPIC_API_KEY` (an API key; gh-aw does not accept a Claude
subscription OAuth token), and every other engine wants its own key. The
invariant and the feature are mutually exclusive, which is the thing to know
before reasoning about either.

They now run `engine: claude` on `ANTHROPIC_API_KEY`, with the model chosen per
workflow by whether the agent exercises judgement:

| Workflow | Model | Why |
|---|---|---|
| `dependency-upgrade-pilot` | `claude-sonnet-5` | verdicts a version range |
| `ci-triage` | `claude-sonnet-5` | classifies a red run |
| `generated-artifact-healer` | `claude-haiku-4-5` | re-runs a known generator |
| `skill-freshness-auditor` | `claude-haiku-4-5` | greps prose against code |

Haiku is the cheapest current model and fine for mechanical work; it is the
wrong default for the two judgement agents, because a confidently wrong verdict
on a dependency bump is the failure #376 already cost three red PRs for. **Never
set `effort` on a Haiku workflow** — Haiku 4.5 rejects the parameter.

One consequence of leaving Copilot: `GH_AW_MAX_DAILY_AI_CREDITS` (5000) is
denominated in Copilot credits and does not bind a BYO-key run, so the spend
ceiling is whatever limit is configured Anthropic-side. These fire on every
dependabot push and every red CI run, so that limit is load-bearing rather than
belt-and-braces.

They fail **closed**, which is the one thing that went right: `activation`
failing leaves `agent`, `detection` and `safe_outputs` all `skipped`, so a
missing or revoked key costs red runs, never an unintended write.

Two interactions with the gates above worth knowing. `rust.yml`'s `detect`
counts these paths as agent-only, so a prompt edit skips the backend gate —
which is safe precisely because the always-on `skills` job is not gated on
`detect` and still catches generated-reference drift. And the SHA-pin rule
applies to the generated lock files too; `gh aw compile` pins them for you.

## The dispatched agent (`agent-task.yml`)

`agent-task.yml` is the fire-and-forget agent: `workflow_dispatch` only, one
task per run, ending in a pull request. It is a plain workflow running
`anthropics/claude-code-action`, not gh-aw, because it authenticates with a
Claude subscription token. It copies gh-aw's split instead. The `agent` job
holds only the workflow token, with `contents: write`, and checkpoints to
`agent-wip/<task>`. A separate `publish` job, on a fresh runner that never
checks out the agent's tree, mints a GitHub App token and opens the PR. Design
and rationale: `docs/agent-fleet.md`.

It carries two stored secrets beyond the gh-aw agents' `ANTHROPIC_API_KEY`, and
unlike that one they are **environment-scoped**: `CLAUDE_CODE_OAUTH_TOKEN` in the
`agent` environment and `AGENT_APP_KEY` in `agent-publish`, both restricted to
`main`, so a PR branch that edits a workflow cannot read them. Do not move either
to repository scope, and do not let the `agent` job reference `AGENT_APP_KEY`.

**Its `agent` job must not gain `id-token: write`.** `er-ci-tf-plan` trusts any
ref of this repository, so a job that can mint an OIDC token can read Terraform
state. That is why it restores `rust-cache` but not the S3 sccache, and why its
`CARGO*`/`RUST*` env must stay identical to `rust.yml`'s `lint` job.

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
