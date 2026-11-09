---
name: land-pr
description: Drives a pull request from opened to green on `main`, then closes its post-merge ledger — the PR loop, the main loop for the push-only workflows no PR can run, the false greens `gh pr checks` reports on a PR that compiled nothing, the house merge rules (no auto-merge; a single-commit PR squashes to the commit subject; only the merge bus judges a combination), and the ledger ticked in the merged body as the record the follow-ups happened. Use when asked to monitor, watch, babysit or land a pull request, when its checks are red or stuck pending, when a failure may be the account rather than the diff, when a merge turns `main` red, or when a merged PR's ledger has open boxes. For why a job fails see `ci-and-supply-chain`; for the local gates see `rust-conventions` and `frontend-testing`; for the card of the issue it closes see `project-board`; for claiming the ticket and the draft PR see `claim-ticket`; for reviewing the diff see `pr-review`.
argument-hint: "[pr-number-or-url] [AUTO_MERGE=true|false] [WATCH_WINDOW=30m]"
---

# Landing a pull request

Arguments: `$ARGUMENTS` — a PR number or URL, then any overrides.

Defaults: `AUTO_MERGE=true`, `WATCH_WINDOW=30m`.

## Invoking this skill is the authorisation

**Being handed a PR here means taking it to green on `main` and closing its
post-merge ledger, not reporting on its progress.** The whole arc is
pre-authorised — commit a fix, push it, `--force-with-lease` over your own
commits after a rebase, rerun a flaky job, squash-merge on green, edit the
merged PR's body to record the ledger, open a follow-up PR, revert if the depth
cap is hit — and none of it needs a per-step yes. Phase boundaries are not
checkpoints: merging and then asking whether to watch `main` is the same as not
having run Phase 3.

The failure this prevents is subtle, because it looks diligent. A procedure that
reports at every boundary and waits is slower than no procedure at all — the
user is now polling the assistant that was asked to do the polling, and the
false green this skill exists to defeat is exactly what they will accept while
doing it.

So the asking is inverted. **Stopping needs a reason**, and
[the list below](#blocked--stop-and-say-why) is that list; it is short and every
entry is either irreversible, environmental, or a judgment that is genuinely the
user's. Everything not on it is yours to do. If the user is unreachable at a
real block, do the preparatory work — branch, diagnosis, draft PR — and stop
there with it done.

Progress still gets reported, one line per state change (check failed →
diagnosing, pushed fix, merged, main green). That is narration of work in
flight, not a request to continue.

Budgets: 5 fix iterations per PR, 2 reruns per distinct flaky failure, 2
follow-up PRs before reverting. Do **not** budget wall-clock per CI pass from a
guess — a cold compile after a dependency bump legitimately runs far longer than
the steady state, and the per-job timeouts are generated into
[`../ci-and-supply-chain/references/required-checks.md`](../ci-and-supply-chain/references/required-checks.md).
A job still queued past its own timeout is the account, not the branch.

Two loops and a ledger. The PR loop ends at green on the branch; the main loop
ends at green on `main` **with this change on it**. The second exists because
`main`'s protection is `strict: false`, so a PR merges on a green it won against
a base that may since have moved, and because `images`, `deploy`, `kind` and
`drift` never run on a pull request at all.

The ledger is what survives the session. Every PR body carries a
`<!-- BEGIN POST-MERGE -->` block from
[`.github/pull_request_template.md`](../../../.github/pull_request_template.md),
and Phase 4 ticks it **in the merged body** with the evidence inline. Without it
a merged PR and an abandoned post-merge watch look identical from GitHub — which
is the state this skill's own observations used to end in.

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
   the only mechanism. Not to be confused with this skill's own `AUTO_MERGE`,
   which is about whether *you* pause before merging, not about a GitHub feature
   — the names collide and the facts are unrelated.
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
| `pull request contract` red | the body closes no issue, or its ledger block was deleted | Fix the **body**, never the diff: add `Closes #<n>`, or restore the ledger from the template. An `edited` event re-runs it with no push |
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

Squash, which is the house method. No confirmation step: `AUTO_MERGE` defaults
true and the invocation was the authorisation. Pausing here to ask is the
behaviour that needs requesting — `AUTO_MERGE=false` is how a user asks for it,
and then the exit is "green, awaiting merge" with the merge command written out
ready to run.

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

Record `MERGE_SHA` in the ledger's first box as you go rather than at the end.
Every later observation attaches to something, and the ledger is the only place
that outlives the session.

## Phase 3 — the main loop

The post-merge chain is two hops and paths-filtered at the first:
`merge → images (only if image inputs changed) → deploy (via workflow_run, only
if images succeeded)`. These consequences decide what "done" means, and
[`references/post-merge.md`](references/post-merge.md) carries them with the
generated list of what runs only after a merge:

- **No deploy run is often correct.** A merge touching only `.claude/` or
  Terraform builds no image, so nothing deploys. Waiting for a run that will
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

## Phase 4 — close the ledger

Everything Phase 3 observed now goes into the PR body, where the next session
can read it. **A merged pull request's body stays writable** — probed
2026-11-07, a `PATCH` on a merged PR returns 200 — so the record belongs there
and not in a transcript.

Read, modify, write. Never compose a body from memory: a concurrent session or
a human may have edited it, and `--body-file` replaces the **whole** body.

```bash
gh pr view <n> --json body --jq .body > /tmp/pr-<n>-body.md
# tick the boxes in that file, evidence inline, then:
gh pr edit <n> --body-file /tmp/pr-<n>-body.md
```

Five rules. Three of them are the annotations, and the asymmetry between which
ones tick is the whole mechanism:

1. **Evidence inline, not a bare tick.** `- [x] main is green — 2860240d, run
   19284756 (22 success / 0 failure)`. A tick with nothing after it is
   indistinguishable from a tick applied to make the list go away.
2. **`n/a — <why>` ticks.** Most ledgers have several: a `.claude/`-only change
   builds no image and deploys nothing, and saying so *is* the finding.
3. **`deferred — #<ticket>` ticks, and the number is not optional.** For work
   that is real, was not done here, and is owned by a filed ticket — tracked
   work rather than missing work, so the audit should stay quiet about it. A
   deferral naming no ticket has retired an item into nothing, which is why
   `pr-ledger-audit.sh` reports it as `LOOSE-DEFER` and the classifier counts
   it. Do not reach for this to clear a box you simply did not get to: file the
   ticket first, then the annotation is true.
4. **An item you could not establish leaves its box EMPTY**, annotated
   `blocked — <why>`, and is named in the final report. This is the one that
   keeps the record worth having: an empty box means exactly one thing, and it
   only keeps meaning that while every other outcome has somewhere else to go.
   Ticking something unverified is the failure that makes the whole mechanism
   worthless — every future reader would have to re-derive what the ledger
   claims to settle.
5. **Items discovered during the watch go under "Specific to this change"**,
   ticked if they were done here and left open if they are owed. That is how a
   one-off backfill or a required-check promotion stops living in chat.

Then set the **Closed by** line to the session that did the work, the machine it
ran on, and the UTC time:

```bash
session_ref="${CLAUDE_CODE_SESSION_ID:+Claude Code:$CLAUDE_CODE_SESSION_ID}"
session_ref="${session_ref:-${CODEX_SESSION_ID:+Codex:$CODEX_SESSION_ID}}"
session_ref="${session_ref:-unavailable}"
printf 'agent · session `%s` on `%s` · %s\n' \
  "$session_ref" "$(hostname -s)" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
```

**Record the agent and host with the id.** A transcript lives on the machine
that ran it, so an id without a host cannot be resolved across machines. Use
`CLAUDE_CODE_SESSION_ID` for Claude Code and `CODEX_SESSION_ID` for Codex; on
that host, reopen the session with `claude --resume <id>` or `codex resume <id>`.
Claude Code also has `claude --from-pr <n>` while that link survives. Do not
infer a Claude Code id from a scratchpad path, which may carry the parent's id
when `CLAUDE_CODE_CHILD_SESSION=1`. If neither variable is available, record
`unavailable` rather than inventing an id.

**Exit** when the ledger has no empty box that is not annotated `blocked`. A
body edit fires `pull_request edited`, so `pull request contract` re-runs on the
merged PR and passes — `pr-contract.yml` checks out the **head SHA** precisely
because a merged PR no longer has the `refs/pull/<n>/merge` ref that
`actions/checkout` otherwise defaults to.

To find the ledgers an interrupted session left open:

```bash
bash scripts/ci/pr-ledger-audit.sh          # LIMIT=100 for a deeper sweep
```

## Blocked — stop and say why

This is the **whole** list. Nothing outside it is a reason to hand control back
mid-procedure.

- An environmental failure — Actions billing, a missing secret, a quota
- A fix that would require weakening a test, a lint or a workflow
- A budget exhausted — the three stated above
- The PR's own change looks wrong, in that the fix the gate demands contradicts
  what the PR set out to do. Say so; do not quietly reshape a feature to satisfy
  a test
- Any Terraform plan that destroys or replaces stateful resources — a database,
  a PVC, a bucket. A fix-forward must never slide a destructive plan through
- A deployment environment waiting on a human approver. Report it; never try to
  approve a deployment
- `AUTO_MERGE=false` was passed, which is a user asking for the pause

Ask once, framed so that yes or no answers it.

**Not blocks, and not worth a question:** a red check (diagnose it), a flaky
check (rerun it, twice at most), a conflicted branch (rebase it), a merge on
green, a `main` that went red after the merge (attribute it, then fix forward),
a long CI pass (keep polling — the user asked for the polling), reaching a
phase boundary, a red `pull request contract` (edit the body), or a ledger item
that cannot be established (annotate the box `blocked` and report it). A question about any of these spends a turn to learn something
the skill already answers.

## Final report

```
## land-pr: #<n> <title>
**Outcome:** merged, main green & ledger closed | green, awaiting merge | stopped — needs human

**PR loop:** <k> iterations
- iter 1: <context> failed (<cause>) → fixed in <sha>
- iter 2: <context> flaky (<reason>) → rerun, passed

**Merge:** <sha> via squash at <time>

**Main loop:**
- <workflow>: green
- deploy: converged | not triggered (no image inputs changed) | <what broke>
- <workflow>: failed → attributed here → follow-up #<m>, merged <sha>, green

**Ledger:** closed in the PR body — <t> ticked (<k> of them `n/a`), <b> blocked
- blocked: <item> — <why, and who can unblock it>

**Commits and PRs created:** <every sha and number>
**Open items:** <or "none">
```

List every commit and PR created, so the work can be audited without reading the
transcript. The ledger line is the pointer to where that audit now lives: the
body, which outlasts both the transcript and this report.
