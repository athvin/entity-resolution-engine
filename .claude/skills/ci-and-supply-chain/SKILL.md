---
name: ci-and-supply-chain
description: Explains this repo's CI topology and dependency gates — the `detect`-job pattern that lets a required check skip without blocking a PR, why no required workflow may ever carry a `paths:` filter, the disk-reclaim step the Rust test job needs, the SHA-pinning policy for actions, and the supply-chain gates (cargo-deny advisories/licences/bans, pnpm audit with its reasoned allowlist, and the licence list mirrored between Rust and npm). Use when a CI job fails for reasons unrelated to the diff, when adding or re-wiring a workflow or job, when a dependency bump cannot go green, when an advisory or licence gate fires, or when deciding whether a check should be required. For local gates see rust-conventions; for the frontend test tiers see frontend-testing.
---

# CI and the supply chain

Four workflows: `rust`, `frontend`, `infra`, `images`. Seven jobs across the
first two are required status checks, so a change is not mergeable until they
pass. The per-job contexts, timeouts and conditions are generated:
[references/required-checks.md](references/required-checks.md).

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

So the PR stays mergeable at near-zero compute. `infra.yml` *does* carry a real
`paths:` filter — and that is fine precisely because it is **not** a required
check.

Both `detect` jobs use `grep -c`, not `grep -q`. `-q` closes the pipe as soon
as it matches, which can SIGPIPE the `git diff` feeding it under `pipefail`, and
the result is a flaky **false skip** — the worst failure mode available, because
it skips the gate rather than failing it.

`rust.yml`'s detect skips the compile jobs only for PRs that touch nothing
outside `.claude/**`. The skip is deliberately that narrow: `docs/` changes
still run the gate, because the lint job verifies generated docs.

## Why a job fails when your diff is innocent

Three recurring causes, worth checking before debugging the change:

**1. Supply-chain gates are time-dependent.** `cargo-deny` and `pnpm audit`
fail on advisories and yanked releases published **since the last green run**.
Nothing in your diff caused it.

Read the **job log**, not the diff — the log names the advisory. Fix it in its
own small PR rather than entangling it with a feature, so the feature's review
stays about the feature.

**2. Disk exhaustion in the Rust `test` job.** A full `rust/target` is tens of
gigabytes, and the job carries a **disk-reclaim step** (it is the only job that
does) which deletes preinstalled toolchains to recover space.

The symptom is misleading: **`linking with cc failed` usually means the disk is
full**, not that anything is wrong with the link. If you see it, check whether
the reclaim step is still present and sufficient.

**3. A cold rebuild after a dependency bump.** The compile jobs share an sccache
cache and link with mold, but a `duckdb` bump invalidates the cache and forces a
full rebuild — which is why the lint and DuckLake e2e jobs carry longer
timeouts than their steady-state runtime needs.

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

**A dependabot PR cannot go green.** Check whether it is `arrow` or
`datafusion`: both are pinned to another crate's major (`duckdb`'s and
`ff-analysis`'s respectively), so an independent bump is unmergeable and the
unblocker is upstream. Dependabot does not currently exclude them, so close
such PRs rather than trying to fix them.

**Adding a gate.** Decide whether it should be required, and make it report on
every PR if so (the `detect` + `if:` pattern). Then **observe it failing**
before trusting it, and give it a rename-detection exit so it cannot pass by
examining nothing.
