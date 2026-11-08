# After the merge: what runs, what does not, and how to tell

The push-to-main surface a pull request can never exercise, how to verify a dev
deploy actually took, and the revert path. Reached from
[`../SKILL.md`](../SKILL.md).

## Contents

- [The chain, and where it stops](#the-chain-and-where-it-stops)
- [Workflows that run after a merge](#workflows-that-run-after-a-merge-to-main) (generated)
- [Verifying the deploy rather than the workflow](#verifying-the-deploy-rather-than-the-workflow)
- [Closing the ledger](#closing-the-ledger)
- [Attribution](#attribution)
- [Reverting](#reverting)

## The chain, and where it stops

```
merge to main
  └─ images       — only if something that goes INTO an image changed
       └─ deploy  — via workflow_run, only if images concluded success
            └─ blue/green rollout into the `ci` namespace
```

What follows decides what "green on main" means (this list grows; do not reason
from its length).

**`images` is paths-filtered, and that is safe here.** It is not a required
check, so the rule against `paths:` on a required workflow does not apply. The
consequence for this loop is that a merge touching only `.claude/` or
`infra/terraform/` builds nothing and deploys nothing — correctly. Waiting for a
deploy run that will never be created is the ordinary way the main loop hangs.

**A platform-only change takes the other door.** `infra/k8s/**` changes nothing
that goes into an image, so `images` never runs and `workflow_run` never fires —
while the script that applies those values lives inside the deploy job. That is
why `deploy` carries its own trigger for those paths. A change that merges green
and is simply never applied is the failure this door exists to close; it was
found by merging a collector allowlist that was committed, green, and absent
from the cluster.

**A cancelled `images` run produces no deploy, and a cancelled run is not a
failure.** Nothing reports it. `images` cancels in progress, so a merge landing
soon after another supersedes the earlier build; the newest run completes and
deploys. `deploy` itself does **not** cancel in progress — deploys queue — which
is what stops a merge from killing a rollout half-applied.

**`workflow_run` runs carry the triggering SHA.** So
`gh run list --commit $MERGE_SHA` does find the deploy, but it appears late —
keep polling until nothing on that SHA has an empty `conclusion`, rather than
concluding from one pass. Poll on the conclusion rather than on a list of
statuses: a run held by a concurrency group reads `pending`, which slips past
`queued`/`in_progress`. It is one of the false greens in `polling.md`, beside
this one and reached from the skill body.

**Your merge SHA's `rust` and `frontend` runs may never run at all, and that is
not about your change.** `cancel-in-progress` is `${{ github.event_name ==
'pull_request' }}` in both workflows, so a push to `main` does **not** cancel
the run in flight — but the concurrency group still admits only one *waiting*
run, so each new merge supersedes the previously **pending** one. During a burst
of merges the first and last runs complete and the ones between are cancelled
having created **zero jobs**.

Measured on `main` during a run of skill PRs: of 8 consecutive `rust` runs, 5
were `cancelled` with `jobs=0` and the 3 survivors had 19–22. So:

- A `cancelled` run with no jobs on your merge SHA is **not** a failure and not
  attributable to your diff. Do not debug it.
- **Re-running it does not help** while merges keep arriving. Observed once:
  `run_attempt` went to 2 — which is the proof a re-run took, not the exit
  code — and attempt 2 was superseded the same way, again with zero jobs.
- So establish "`main` is green with my change on it" another way: a **later**
  head SHA's run that has your commit as an ancestor
  (`git merge-base --is-ancestor $MERGE_SHA origin/main`), or — strongest when
  the change's only applicable gate runs locally — the gate itself against
  `origin/main` in a throwaway worktree:

  ```bash
  git worktree add --detach /tmp/main-verify origin/main
  cd /tmp/main-verify && make skills-check    # or the gate your diff touches
  ```

<!-- BEGIN GENERATED: post-merge-surface -->
### Workflows that run after a merge to `main`

| Workflow | Triggers |
|---|---|
| `deploy` | `workflow_dispatch workflow_run` |
| `devcontainer` | `pull_request push workflow_dispatch` |
| `drift` | `schedule workflow_dispatch` |
| `frontend-experiments` | `schedule workflow_dispatch` |
| `frontend` | `merge_group pull_request push schedule workflow_dispatch` |
| `images` | `pull_request push workflow_dispatch` |
| `infra` | `merge_group pull_request push` |
| `kind` | `pull_request schedule workflow_dispatch` |
| `rust` | `merge_group pull_request push schedule workflow_dispatch` |

### Of those, the ones no pull request can pre-test

These carry no `pull_request` trigger, so a green PR says nothing about
them. **This is the surface the main loop exists for.**

- `deploy` — `workflow_dispatch workflow_run`
- `drift` — `schedule workflow_dispatch`
- `frontend-experiments` — `schedule workflow_dispatch`
<!-- END GENERATED: post-merge-surface -->

One caveat the table cannot express: a `pull_request` trigger does not mean the
**same jobs** ran. `kind` triggers on pull requests, but its PR leg is a
one-second fixture test while the real cluster probe is nightly; `rust` and
`frontend` likewise carry nightly-only tiers. So a workflow absent from the
second list can still fail after a merge in a job no PR ever executed.

## Verifying the deploy rather than the workflow

A green deploy workflow is not a running change. Three distinct ways it lies:

- **A rejected configuration crashloops the new pod while the old one keeps
  serving**, so the rollout can report success over an unchanged service. A
  ConfigMap landing is not a configuration being accepted.
- **The image digest proves a build, not a process.** Verify by a log line the
  running process itself emits — a startup banner naming the new behaviour, a
  span, a metric that only the new code path produces.
- **The rollout targets one colour.** The deploy is blue/green into the `ci`
  namespace: it rolls `web-api-<target>`, smoke-tests the preview service's
  health endpoint, cuts the service over, then scales the old colour to zero.
  Reading the live deployment while the preview is the one that moved shows no
  change at all.

With cluster access:

```bash
kubectl -n ci get deploy -l app.kubernetes.io/name=er-platform
kubectl -n ci rollout status deploy/web-api-<colour> --timeout=10m
kubectl -n ci get pods --field-selector=status.phase!=Running
kubectl -n ci logs deploy/web-api-<colour> --since=10m | grep <the line the new code emits>
```

The end-to-end cutover has never been exercised as a test, so treat a colour
swap as the part of this chain most likely to surprise. Without cluster access,
say so in the report rather than inferring convergence from the workflow
conclusion.

## Closing the ledger

Everything above is an observation, and an observation that stays in a session
transcript is lost. The ledger is where it lands: a checkbox block between
`<!-- BEGIN POST-MERGE -->` and `<!-- END POST-MERGE -->` in the pull request
body, put there by
[`.github/pull_request_template.md`](../../../../.github/pull_request_template.md)
and required by the `pull request contract` check.

**A merged pull request's body is still writable.** Probed 2026-11-07: a `PATCH`
to `/repos/{owner}/{repo}/pulls/806` on a pull request that was already
`merged: true` returned 200. That one fact is what makes the body — rather than
a comment, a transcript, or a report — the right place for the record.

```bash
gh pr view <n> --json body --jq .body > /tmp/pr-<n>-body.md
#   tick each box, evidence inline; `n/a — <why>` where it does not apply;
#   leave a box EMPTY and annotate it `blocked — <why>` if it could not be
#   established, because an unverified tick is worse than an open box
gh pr edit <n> --body-file /tmp/pr-<n>-body.md
```

Read before writing, every time: `--body-file` replaces the whole body, and a
concurrent session or a human may have edited it since the merge.

Each generic item resolves against something earlier in this file:

| Ledger item | What settles it | Where |
|---|---|---|
| `main` green with this change on it | a concluded run on `MERGE_SHA`, or a later head SHA with it as an ancestor — **not** a cancelled run with zero jobs | [the cancellation measurement](#the-chain-and-where-it-stops) |
| `images` built or correctly never ran | the paths filter: a `.claude/`- or Terraform-only merge builds nothing | same |
| `deploy` converged | a log line the running process emitted | [verifying the deploy](#verifying-the-deploy-rather-than-the-workflow) |
| the push-only surface | the generated table of what no pull request pre-tests | [the generated list](#workflows-that-run-after-a-merge-to-main) |
| the board card reached `Done` | the automation, confirmed from the issue's own timeline (`closer.__typename == "PullRequest"`) | the `project-board` skill |
| follow-ups opened | the numbers, or `n/a` | — |
| what was learned is written down | the skill, reference or memory file it went into | — |

The board row is the one worth not shortcutting. `Status` → `Done` set by hand
**closes the underlying issue**, so moving the card yourself both duplicates the
automation and races it; the ledger asks you to confirm the automation acted,
not to act for it.

To find ledgers an interrupted session left open:

```bash
bash scripts/ci/pr-ledger-audit.sh         # LIMIT=100 for a deeper sweep
```

It distinguishes `closed` from `—` (no ledger at all — a pull request merged
before the contract existed, or an exempt one), because both count zero open
boxes and mean opposite things.

## Attribution

```bash
gh pr diff <n> --name-only                        # what this change touched
gh run list --branch main --limit 10 --json conclusion,headSha,workflowName,event
```

Compare against the Phase 0 baseline. Red before `MERGE_SHA` is pre-existing and
gets reported, not fixed here. A failing log naming a path this PR touched is
ours. Ambiguous counts as ours.

Two main-only tiers fail for their own reasons and are worth recognising before
attributing anything to the diff: `drift` compares the account against its state
file, so a red run there is information about AWS rather than about a commit;
and the `kind` probe's nightly leg spent months failing for five separate causes
that all presented as a job timeout, because a Pending pod waits out the budget.
Its first green scheduled run on `main` was 2026-11-07 — so a red one is now
worth attributing rather than discounting, but check the failure mode against
those causes before blaming a diff.

## Reverting

The depth cap is two follow-up PRs. After that:

```bash
git fetch origin && git switch -C revert/pr-<n> origin/main
git revert -m 1 $MERGE_SHA
git push -u origin HEAD
```

`-m 1` is for a merge commit; a squashed merge is an ordinary commit and reverts
without it. Then run the PR loop on the revert PR, merge it, confirm `main` is
green, and write up what was learned so the change can be re-landed properly.

Revert immediately, skipping fix-forward, when the dev deploy is serving errors,
when one diagnosis pass has not produced an obvious fix, or when the user is
away. Availability beats elegance.
