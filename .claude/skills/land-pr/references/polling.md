# Reading a PR's state without being lied to

Every command needed to decide whether a pull request is actually green, and the
six ways the obvious reading is wrong. Reached from
[`../SKILL.md`](../SKILL.md); the failure taxonomy those readings feed into is
`ci-and-supply-chain`'s.

## Contents

- [The one assertion to run before any merge](#the-one-assertion-to-run-before-any-merge)
- [Why `gh pr checks` is not that assertion](#why-gh-pr-checks-is-not-that-assertion)
- [The false greens](#the-false-greens)
- [Reading a failure's cause from execution rather than conclusion](#reading-a-failures-cause-from-execution-rather-than-conclusion)
- [mergeStateStatus, state by state](#mergestatestatus-state-by-state)
- [Polling without hammering the API](#polling-without-hammering-the-api)

## The one assertion to run before any merge

Three questions, in this order. A merge is sound only if the first answers `0`,
the second prints nothing, and the third prints nothing.

```bash
PR=<n>
SHA=$(gh pr view "$PR" --json headRefOid --jq .headRefOid)

# 1. Did any `detect` job FAIL? A failed detect makes every job gated on it
#    report `skipped`, which branch protection accepts as satisfied — so the
#    whole required set can be green with nothing compiled.
gh api "repos/{owner}/{repo}/commits/$SHA/check-runs" --paginate \
  --jq '[.check_runs[]|select(.name|startswith("detect"))|select(.conclusion=="failure")]|length'

# 2. Which required contexts are NOT satisfied on this exact SHA?
#    Takes the LATEST check-run per name, so an old green attempt cannot mask a
#    newer red one. Empty output is the pass condition.
comm -13 \
  <(gh api "repos/{owner}/{repo}/commits/$SHA/check-runs" --paginate --jq '
      [.check_runs[]] | group_by(.name)
      | map(max_by(.started_at))
      | map(select(.conclusion=="success" or .conclusion=="skipped") | .name) | .[]' | sort -u) \
  <(gh api repos/{owner}/{repo}/branches/main/protection \
      --jq '.required_status_checks.contexts[]' | sort -u)

# 3. Is anything still unfinished on this SHA? Ask by CONCLUSION, not status.
#    `status` has more values than the two obvious ones — a run held by a
#    concurrency group reads `pending` — and enumerating them is how a poll
#    reports "all concluded" over a run that has not started.
#
#    Compare against the EMPTY STRING, not null: `gh` serialises an unfinished
#    run's conclusion as `""`, so `select(.conclusion == null)` matches nothing
#    and silently reports the same false green. `// ""` normalises both.
gh run list --commit "$SHA" --json workflowName,status,conclusion \
  --jq '[.[]|select((.conclusion // "") == "")]'
```

Two properties make this worth the three calls. It asks GitHub for the required
set rather than restating it, so it cannot drift from protection when a context
is added or renamed. And a context that **never reported** shows up in step 2
exactly like a failing one — which is the case `gh pr checks` drops silently.

A context whose name contains `${{ matrix.* }}` can never be required; if one
ever appears in the protection list, that is the bug, not a pending check.

## Why `gh pr checks` is not that assertion

It is still the right tool for a quick human-readable look, and
`--watch --fail-fast` is a reasonable way to sit on a run. It is not the thing
to decide a merge on:

- It does not bucket `queued` as pending, so a PR whose runs are held or not yet
  started can read as passing.
- It reports the checks that exist. A required context that never reported is
  absent from the output rather than listed as missing, and absent reads as
  fine.
- It does not know which contexts are required, so it cannot tell a red
  non-required check (`semgrep`, the nightly tiers) from a red required one.

## The false greens

Each of these has cost real time here, and each reads as a passing PR. **This
list grows — do not reason from its length**, and do not treat "none of these
applies" as proof of green when the assertion above has not been run.

**1. A failed `detect` job.** Covered above and the most dangerous, because the
PR is genuinely `CLEAN` and genuinely mergeable — by a human or by the merge
bus — with nothing built. Seen with two Rust files changed, all three `detect`
jobs dead in an Actions-minutes outage, every required context `skipped`.

**2. `mergeStateStatus: DIRTY`.** A conflicted PR triggers **zero** workflows
and reports no checks at all. Any predicate phrased as "nothing is failing"
reads that as green. Reject `DIRTY` by name.

**3. `UNKNOWN` from a list call.** `gh pr list` returns `UNKNOWN` for
`mergeStateStatus` and does not trigger the computation. Only `gh pr view` does.

**4. A held bot-authored run.** Runs on a bot's PR are created
`conclusion: action_required` with **zero jobs**, so `statusCheckRollup` is
literally empty and `/check-runs` answers HTTP 500. Read held-ness from
`actions/runs?head_sha=…` selecting `conclusion == "action_required"`.

**5. A content-addressed pass skip.** `rust` and `frontend` `detect` compute a
pass key over the tested tree plus the ISO week; a `ci-pass-*` cache hit means
that exact content already went green **on this PR**, so the gate skips. That is
a legitimate green — but it is a green for content, not for this run, so a
`skipped` gate on a PR that has never been built once is worth a second look.
The `ci:full` label forces a full run.

**6. A stacked PR marked `MERGED`.** Deleting the parent's branch marks the
child merged, with a real `mergeCommit`, while its code never reaches `main`.
Confirm with `git log origin/main --oneline -1 <mergeCommit>`, or grep `main`
for a string the PR introduced.

**7. A poll that enumerates statuses instead of asking for a conclusion.** A run
held by a concurrency group reads **`pending`**, which is neither `queued` nor
`in_progress`, so the obvious predicate
`select(.status=="queued" or .status=="in_progress")` reports nothing
outstanding while a run sits unstarted. Measured here: a post-merge watch
printed "ALL CONCLUDED" over a `rust` run that was `pending` and went on to be
cancelled without ever running a job.

Poll `(.conclusion // "") == ""` instead — it is the only predicate that does
not need the status vocabulary to be complete. Note the `// ""`: `gh run list`
and `gh run view` both serialise an unfinished run's conclusion as the **empty
string**, not `null`, so the obvious `select(.conclusion == null)` matches
nothing and reproduces the very false green it was meant to fix. Verified
against a live `pending` run: the null form counted 0 outstanding, this one
counted 1.

## Reading a failure's cause from execution rather than conclusion

A starved run and a legitimately failing one are both `failure`. Only one of
them ran a step:

```bash
gh api "repos/{owner}/{repo}/actions/runs/<run_id>/jobs" --jq '[.jobs[].steps|length]|add'
```

Zero across every job means no runner was ever allocated — the account is out of
Actions minutes, nothing in the repository can go green, and it is an account
setting rather than a repository one. Measured here: starved runs summed to 0,
real ones to 5, 5 and 202.

The per-step conclusions are the other thing worth reading, because a step count
alone can mislead in the opposite direction:

```bash
gh run view <runId> --log-failed
gh run view <runId> --json jobs \
  --jq '.jobs[]|select(.conclusion=="failure")|{name,steps:[.steps[]|select(.conclusion=="failure")|.name]}'
```

## mergeStateStatus, state by state

| State | Means | Do |
|---|---|---|
| `CLEAN` | every required check satisfied, review not required here | run the assertion above, then merge |
| `BEHIND` | base moved | benign — protection is `strict: false`. Rebase only if the change interacts with what moved |
| `DIRTY` | conflicted | rebase onto `origin/main`, resolve, push; **zero CI ran**, so a full pass follows |
| `UNSTABLE` | a non-required check is red *or still pending* | usually transient. `semgrep` red is mergeable by hand; the merge bus refuses it |
| `BLOCKED` | a required context is failing or absent | step 2 of the assertion names which |
| `UNKNOWN` | not computed | re-read with `gh pr view`, never `gh pr list` |

## Polling without hammering the API

60–120s between passes. A gate cycle here is minutes at best and much longer on
a cold compile, so a tighter loop buys nothing and burns rate limit.

Prefer one `gh run list --commit <sha>` per pass over per-run calls, and only
descend into `gh run view` once something has concluded `failure`. When a run is
expected to take a known long time — a cold dependency bump, the e2e tiers —
match the poll interval to that, not to impatience.
