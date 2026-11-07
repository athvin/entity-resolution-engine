---
name: land-pr
description: Drives a pull request from opened to green on `main` — the PR loop, the main loop for the push-only workflows a PR can never run, the false greens that make `gh pr checks` report success on a PR that compiled nothing, and the house merge rules (no auto-merge, a single-commit PR squashes to the commit subject rather than the PR title, `main` is `strict: false` so only the merge bus judges a combination). Use when asked to monitor, watch, babysit or land a pull request, when its checks are red or stuck pending, when deciding whether a failure is the diff or the account, or when a merge turns `main` red. For why a job fails and the CI topology see `ci-and-supply-chain`; for the local gates to reproduce against see `rust-conventions` and `frontend-testing`; for the card of the issue it closes see `project-board`; for claiming the ticket and opening the draft PR see `claim-ticket`; for reviewing the diff before landing it see `pr-review`.
argument-hint: [pr-number-or-url] [AUTO_MERGE=true|false] [WATCH_WINDOW=30m]
disable-model-invocation: true
---

# Landing a pull request

Arguments: `$ARGUMENTS` — a PR number or URL, then any overrides.

Defaults: `AUTO_MERGE=true` (there is a standing authorisation to push, open and
merge on green here, so stopping at green to ask is the behaviour that needs
requesting, not the merge), `WATCH_WINDOW=30m`.

Budgets: 5 fix iterations per PR, 2 reruns per distinct flaky failure, 2
follow-up PRs before reverting. Do **not** budget wall-clock per CI pass from a
guess — a cold compile after a dependency bump legitimately runs far longer than
the steady state, and the per-job timeouts are generated into
[`../ci-and-supply-chain/references/required-checks.md`](../ci-and-supply-chain/references/required-checks.md).
A job still queued past its own timeout is the account, not the branch.

Two loops. The PR loop ends at green on the branch; the main loop ends at green
on `main` **with this change on it**. The second exists because `main`'s
protection is `strict: false`, so a PR merges on a green it won against a base
that may since have moved, and because `images`, `deploy`, `kind` and `drift`
never run on a pull request at all.

Report state changes in one line each — check failed → diagnosing, pushed fix,
merged, main green. Not every poll.

## The six house rules that override the generic procedure

1. **`gh pr checks` reports false greens.** It does not bucket `queued` as
   pending, and a required context that never reported is simply absent from its
   output. Judge the PR by counting required contexts against the current head
   SHA — [`references/polling.md`](references/polling.md) carries the commands.
2. **A green with a failed `detect` job verified nothing.** Every job gated on
   `detect` reports `skipped` when `detect` *fails*, and branch protection
   accepts `skipped`. The whole required set can be satisfied with nothing
   compiled. Assert against this before every merge.
3. **`mergeStateStatus: DIRTY` runs zero workflows** and reports no checks.
   "No checks reported" is never green.
4. **There is no auto-merge.** `allow_auto_merge` is false on this repository, so
   `gh pr merge --auto` cannot arm anything; the poll-then-merge loop below is
   the only mechanism.
5. **`make check` is the local gate, not the cargo trio.** `cargo fmt` +
   `clippy` + `test` skip `entities-check` and `skills-check`, which is where a
   change that looks clean actually fails. See `rust-conventions`.
6. **Do not race the merge bus.** With several PRs open, hand-merging a
   candidate out from under an in-flight batch discards a gate run the batch
   already paid for. If a batch is running, add the label and let it land the PR.
   See `ci-and-supply-chain`.

## Phase 0 — record the base before touching anything

```bash
gh pr view <n> --json number,title,headRefName,baseRefName,headRefOid,mergeable,mergeStateStatus,isDraft,url,files
gh api repos/{owner}/{repo}/branches/main/protection --jq '{strict:.required_status_checks.strict,contexts:.required_status_checks.contexts}'
gh run list --branch main --limit 10 --json conclusion,headSha,workflowName,event
```

Branch protection is repository configuration, not a file in the tree, so that
API call is the only authority on the required set. Review is not a gate here —
`required_approving_review_count` is zero, which is what makes
`mergeStateStatus == CLEAN` equivalent to "every required check is satisfied".
Never self-approve, and never wait on a review this repo does not require.

The `main` run list is the attribution baseline. When `main` is red after the
merge, this is the only thing that distinguishes "I broke it" from "it was
already broken".

## Phase 1 — the PR loop

**Poll** every 60–120s. CI is not faster because you asked. Each pass, re-read
`mergeStateStatus`: `BEHIND` is benign here (`strict: false`), `DIRTY` needs a
rebase, and a rebase is a code change that needs a full pass afterwards.

**Diagnose before touching code.** Read the failing step's log, then classify —
and check the account first, because it is the only cause that has nothing to do
with the branch:

| Signature | Cause | Action |
|---|---|---|
| Every job fails in seconds, `steps: []`, log is a `BlobNotFound` 404, every branch red | Actions billing | Stop. Tell the user; no diff fixes this |
| `detect` red, everything else `skipped`, `CLEAN` | the false green | Re-run `detect`; never merge on it |
| cargo-deny or pnpm audit red with an innocent diff | an advisory published since the last green | Fix in **its own PR**, never entangled with a feature |
| `linking with cc failed` | the runner disk filled | Check the disk-reclaim step |
| `DirectoryNotEmpty` on a DuckDB extension dir | a stripped cache sentinel, not a dirty `OUT_DIR` | A `prefix-key` bump cannot fix it |
| Same job red on `main`'s latest run | pre-existing | Report it; do not fix unrelated breakage here |

The taxonomy behind that table, and what to do about each, is
`ci-and-supply-chain`'s subject — read it rather than re-deriving.

**Reproduce locally before pushing a fix.** A fix watched passing beats one
reasoned about, and it saves a full gate cycle:

| Change touches | Run |
|---|---|
| anything under `rust/` or `entities/` | `make check` |
| a lake or Postgres path | `make check-e2e` |
| anything under `frontend/` | `make frontend` |
| anything under `.claude/` | `make skills-check` |

Two traps when running these by hand: **put the command of interest last** in a
compound invocation, or a trailing `tail`/`grep` masks its exit code and a red
gate reports success; and `er-entity` embeds its own rule registry, so a
prebuilt binary from another branch reports a false "stale generated file" —
let `make` build it.

**Start fixing at once, but hold the push until the run finishes.** Begin
diagnosing and fixing locally the moment a job goes red — that work is free. Do
**not** push while other required jobs are still running. Two reasons, and the
second is the expensive one:

- A later job may fail too, and one push should carry every fix that run
  earned. Pushing per-failure spends a fix iteration on each.
- **`cancel-in-progress` is true for `pull_request`**, so a push cancels the
  in-flight run. The jobs that had not yet reported never do, and their evidence
  is gone — so you cannot know whether there was more to fix, and the next run
  has to rediscover it from scratch. Measured on this skill's own PR: a push
  made while `rust` was mid-flight cancelled one test shard and left eight e2e
  jobs with no conclusion at all.

Wait for the run to reach a conclusion, collect **every** failure, then push
once. The exception is a failure that makes the rest of the run meaningless —
`detect` red, or a compile error every downstream job is blocked on.

**Fix.** The smallest change that addresses the root cause, committed on the PR
branch with a message saying what was failing and why this fixes it. Push
normally; `--force-with-lease` only over your own commits after a rebase, never
over commits you did not write.

Before any commit, in the **same** tool call as the commit: confirm
`git branch --show-current` and `git rev-parse HEAD`. Checkouts here are shared
with concurrent sessions, so stage by explicit path and never `git add -A`.

**These are not fixes**, even though they turn a check green: deleting or
weakening the failing assertion; `#[ignore]`, `test.skip`, `xit`;
`continue-on-error: true`, removing a job, dropping a required context;
loosening clippy/eslint/tsc, `#[allow(...)]` on new code, `// @ts-ignore`;
swallowing an error; raising a timeout on a test that is actually hanging. If
one of those is the only route to green, that is a finding — stop and say what
the test was catching. Updating a test that encodes behaviour this PR
deliberately changed is legitimate; say so explicitly in the commit message.

**Reruns.** `gh run rerun <id> --failed`, twice at most per distinct failure.
Twice the same way is not flaky, it is real. Two local traps make a rerun lie:
an isolated Playwright spec passes alone while the full tier fails, and the
fast test tiers pass while the integration tier catches the cross-module leak.
`run_attempt` incrementing is the proof a rerun took — not the exit code.

**Exit** when every required context is `success` or `skipped` **on the current
head SHA**, no `detect` run failed, `mergeStateStatus` is `CLEAN`, and the PR is
not a draft.

## Phase 2 — merge

Only with `AUTO_MERGE=true` or an explicit yes. Squash, which is the house
method.

**The squash subject trap.** `squash_merge_commit_title` is
`COMMIT_OR_PR_TITLE`: a **single-commit** PR squashes to the *commit* subject
and silently ignores the PR title. So retitling a PR to add a marker the
tree-destroy guard reads — `ALLOW-DESTROY` — does nothing. Pass it explicitly:

```bash
gh pr merge <n> --squash --subject "<the subject that must land>"
```

Target the number `gh pr create` returned. **Never predict a PR number** —
guessing one has merged an unrelated PR here.

Then capture the merge SHA and verify the code actually landed, because
`state: MERGED` is not evidence: a stacked PR whose parent branch was deleted is
marked merged with a real `mergeCommit` while its code never reached `main`.

```bash
MERGE_SHA=$(gh pr view <n> --json mergeCommit --jq .mergeCommit.oid)
git fetch origin main && git log origin/main --oneline -1 "$MERGE_SHA"
```

Then say, explicitly, that it merged **and that the main loop is now running.**
This is the moment the user would otherwise assume the work was finished.

## Phase 3 — the main loop

The post-merge chain is two hops and paths-filtered at the first:
`merge → images (only if image inputs changed) → deploy (via workflow_run, only
if images succeeded)`. These consequences decide what "done" means, and
[`references/post-merge.md`](references/post-merge.md) carries them with the
generated list of what runs only after a merge:

- **No deploy run is often correct.** A merge touching only `docs/`, `.claude/`
  or Terraform builds no image, so nothing deploys. Waiting for a run that will
  never exist is the common way this loop hangs.
- **A cancelled `images` run produces no deploy and is not a failure**, so
  nothing reports it. Merged does not mean deployed.
- **Your merge SHA's `rust`/`frontend` run may be cancelled with zero jobs**, and
  that is the concurrency group superseding a *pending* run as the next merge
  lands — not your diff, and not fixable by re-running. Establish green from a
  later head SHA that has your commit as an ancestor, or by running the gate
  against `origin/main` directly.
- **A green deploy is not a running change.** Verify by a log line the process
  itself emits — not the image digest, not the workflow conclusion. A rejected
  config crashloops the *new* pod while the old one keeps serving, which is a
  green deploy over an unchanged service.

**Attribute before acting.** Already red before `MERGE_SHA` → pre-existing,
report only. Failing log references a path in `gh pr diff <n> --name-only` →
yours. Ambiguous → assume yours; a wrong follow-up PR is cheap and a red `main`
is not.

**Fix forward,** branching from the base rather than the PR branch:

```bash
git fetch origin && git switch -C fix/pr-<n>-post-merge-<desc> origin/main
# fix, then push and open the PR, and re-run this whole procedure on it
```

`switch -C <name> origin/main` is the spelling that repoints the upstream;
`push -q` hides a push that did not land, so do not use it.

**Depth cap: two follow-up PRs.** If the second also breaks `main`, stop fixing
forward and revert: `git revert -m 1 $MERGE_SHA` on a branch, run the PR loop on
the revert PR, merge it, confirm `main` is green, then write up what was learned
so the change can be re-landed. Revert *immediately*, skipping fix-forward, when
the dev deploy is serving errors, when one diagnosis pass has not produced an
obvious fix, or when the user has said they are away.

**Exit** when every run on `MERGE_SHA` and any follow-up merge SHA is green,
nothing new on `main` has failed for `WATCH_WINDOW`, and any deploy has
converged on a log line the process emitted.

## Stop and ask instead of looping

- A merge without `AUTO_MERGE` or an explicit yes
- An environmental failure — Actions billing, a missing secret, a quota
- A fix that would require weakening a test, a lint or a workflow
- A budget exhausted
- The PR's own change looks wrong, in that the fix the gate demands contradicts
  what the PR set out to do. Say so; do not quietly reshape a feature to satisfy
  a test
- Any Terraform plan that destroys or replaces stateful resources — a database,
  a PVC, a bucket. A fix-forward must never slide a destructive plan through
- A deployment environment waiting on a human approver. Report it; never try to
  approve a deployment

Ask once, framed so that yes or no answers it. If the user is unreachable, do
the preparatory work — branch, diagnosis, draft PR — and stop there.

## Final report

```
## land-pr: #<n> <title>
**Outcome:** merged & main green | green, awaiting merge | stopped — needs human

**PR loop:** <k> iterations
- iter 1: <context> failed (<cause>) → fixed in <sha>
- iter 2: <context> flaky (<reason>) → rerun, passed

**Merge:** <sha> via squash at <time>

**Main loop:**
- <workflow>: green
- deploy: converged | not triggered (no image inputs changed) | <what broke>
- <workflow>: failed → attributed here → follow-up #<m>, merged <sha>, green

**Commits and PRs created:** <every sha and number>
**Open items:** <or "none">
```

List every commit and PR created, so the work can be audited without reading the
transcript.
