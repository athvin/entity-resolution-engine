---
name: claim-ticket
description: Claims a ticket and starts the work in the open — selects the most important unclaimed card in the board's In progress queue (or takes a given issue number), marks the claim where other agents and humans look (assignee, a claim comment, and an early draft pull request pushed from an empty commit so the work is visible on GitHub before any code exists), cuts a worktree from a freshly pulled origin/main, reads the issue and everything it names, and writes the plan into the draft PR body before implementation begins. The claim is the open draft PR — closing it releases the ticket. Use when asked to pick up, claim, grab or start work on a ticket, to take the next most important item, or when asked what to work on next and the answer should become active work. For board field mechanics, ids and the automations see `project-board`; for driving the finished PR from ready to merged and `main` green see `land-pr`.
argument-hint: [issue-number]
disable-model-invocation: false
---

# Claiming a ticket

Arguments: `$ARGUMENTS` — optionally an issue number. Given one, skip selection
but still run the claim check; given none, select as below.

## What a claim is

A claim is three visible marks, made in this order, and the middle one is the
one that holds:

1. **Assignee** on the issue — shows on the board card without opening it.
2. **An open draft pull request** whose body says `Closes #<n>` — visible on
   GitHub, linked from the issue and the card automatically. **This is the
   claim.** It exists before any real code does, carried by an empty commit.
   Closing it releases the ticket; merging it completes it. No separate
   bookkeeping to forget.
3. **A claim comment** on the issue naming the PR and a UTC timestamp — the
   tie-breaker if two agents collide.

Board `Status` is **not** part of the claim. `In progress` is the queue this
skill draws from, not proof anyone is on it — and `Done` must never be set by
hand, because the board automation closes the underlying issue.

## Phase 0 — select

Read the board live (`project-board` owns the mechanics; never answer from
memory, always pass `--limit`):

```sh
gh project item-list 2 --owner athvin --limit 400 --format json --jq '
  .items[] | select(.status=="In progress") | select(.content.number) |
  "\(.content.number)\t\(.priority // "-")\t\(.estimate // "-")\t\(.title)"'
```

Order candidates: `P0` > `P1` > `P2` > no priority; ties break by the stack
rank in the issue body if both carry one, else the lower issue number. Then
walk the list and take the first **unclaimed** one. The claim check, per
candidate:

```sh
gh api graphql -f query='query($n:Int!){repository(owner:"athvin",name:"entity-resolution-engine"){
  issue(number:$n){assignees(first:5){nodes{login}}
    closedByPullRequestsReferences(first:10,includeClosedPrs:false){
      nodes{number isDraft createdAt url}}}}}' -F n=<n> \
  --jq '.data.repository.issue'
```

Any open PR in that list means claimed — skip the candidate. An assignee with
no open PR is a stale or manual claim: skip it too, and note it in the report
rather than stealing it.

If every In-progress card is claimed, **report that and stop.** Do not pull
from `Todo` — promoting a card is a deliberate act, not a side effect of
wanting something to do.

## Phase 1 — claim

**Worktree first**, by the CLAUDE.md ritual: run the audit, refresh `main`,
cut from `origin/main`. Branch `issue/<n>-<slug>`.

The body is built **from the pull request template**, not hand-written, so the
claim satisfies the `pull request contract` check from birth — a ticket link and
a post-merge ledger. Line 1 of the template is the ticket link and nothing else,
which is what makes `tail -n +2` the whole substitution:

```sh
git commit --allow-empty -m "wip: claim #<n> — <short title>"
git push -u origin "$(git branch --show-current)"   # never push -q

session_ref="${CLAUDE_CODE_SESSION_ID:+Claude Code:$CLAUDE_CODE_SESSION_ID}"
session_ref="${session_ref:-${CODEX_SESSION_ID:+Codex:$CODEX_SESSION_ID}}"
session_ref="${session_ref:-unavailable}"
{ printf 'Closes #%s\n\n' "<n>"
  printf '> 🚧 **Claimed by an agent session** — %s, session `%s` on `%s`.\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$session_ref" "$(hostname -s)"
  printf '> The claim is this open draft PR: closing it releases the ticket.\n'
  tail -n +2 .github/pull_request_template.md
} > /tmp/claim-<n>-body.md

gh pr create --draft \
  --title "<type>(<area>): <imperative summary> (#<n>)" \
  --body-file /tmp/claim-<n>-body.md
```

**The session id and host in that banner are the durable link back to the
work.** CLAUDE.md's worktree audit cannot reliably tell which session owns a
worktree — `lsof` misses the dominant pattern, a session whose cwd is the
primary checkout driving the tree with `git -C` — and a claim PR naming its
session and machine answers that from GitHub instead of from the process table.
Use `CLAUDE_CODE_SESSION_ID` in Claude Code and `CODEX_SESSION_ID` in Codex.
On that host, reopen the recorded id with `claude --resume <id>` or
`codex resume <id>`, respectively. Claude Code also has `claude --from-pr <n>`
while that link survives. If neither environment variable is present, record
`unavailable` rather than inventing an id.

Capture the URL `gh pr create` prints — **never predict a PR number.** Then
the other two marks:

```sh
gh issue edit <n> -R athvin/entity-resolution-engine --add-assignee "@me"
gh issue comment <n> -R athvin/entity-resolution-engine \
  --body "Claimed — working in <pr-url> (draft). Closing that PR releases the claim."
```

**Race check.** Re-run the Phase 0 claim query. Two open PRs on one issue
means two agents claimed at once: the earlier `createdAt` wins. If that is not
yours, close your PR with a comment saying so and return to selection.

**Expect the draft to go RED immediately, and leave it red.** The empty commit
produces an empty diff, so `detect` reports `code=false`, every compile job
reports `skipped`, and the `empty pull request` job fails on purpose — measured
at 19s for the whole `rust` run, against 9m54s for a real one. That red is the
claim working as designed: it says "claimed, nothing built yet", and a draft is
unmergeable anyway. The first real push flips `empty` to false and the actual
gate takes over.

**So do not try to fix it, and do not read it as the gate failing.** Two checks
must still be green, because neither is gated on the diff: `agent skills (lint +
drift)`, which runs on everything, and `pull request contract`, which reads the
body — and the body was built from the template precisely so that one passes on
an empty commit.

Before 2026-11-07 this was the opposite advice — the run was the FULL backend
gate and the instruction was to ignore it as green noise, because `rust.yml`'s
`detect` is a denylist and an empty path list inverted through it. Each claim
cost ~10m across the compile matrix and ~0.6 GB of sccache S3 egress to prove a
tree identical to `main`.

## Phase 2 — read, then plan in the open

No code yet. In order:

1. **The issue, completely** — body, checklist, comments, anything it links.
2. **The files and paths it names**, in the tree just cut from fresh `main` —
   a ticket's line numbers and claims may have drifted since it was written;
   the tree is the authority.
3. **The routed skills** — CLAUDE.md's Doing → Skill table says which to load
   for the area being touched.

Then write the plan **into the PR body** (`gh pr edit <pr> --body-file …`):
ordered steps, the files to be touched, the gates that prove it (`make check`,
`make frontend`, or the tier the diff belongs to). The plan goes in the
template's collapsed **Context** block, under `### Plan` — that block exists so
the descriptive material is one click away instead of burying the top of the
body. The PR body is the living document other agents and humans read; the
session transcript is not.

**Read the body and edit it, never regenerate it.** `--body-file` replaces the
whole thing, and three parts of it must survive every rewrite: the `Closes`
line, the claim banner, and the `<!-- BEGIN POST-MERGE -->` ledger. Dropping
either of the first and third turns `pull request contract` red — which is the
gate noticing, correctly, that the claim no longer links its ticket or has
anywhere to record what happens after the merge.

Only then start implementing.

## Phase 3 — work, and hand off

- Commit early and push regularly — a claim PR with no commits for a long
  stretch reads as abandoned. Before every commit, in the **same** tool call:
  `git branch --show-current` and `git rev-parse HEAD`; stage by explicit
  path, never `git add -A`.
- Refresh periodically and before asking for a merge:
  `git fetch origin && git rebase origin/main`. The content-addressed pass key
  makes a rebase over gate-irrelevant movement cheap.
- Keep the PR body's plan current — tick steps off, record discoveries that
  changed it.
- When the local gate is green and the plan is done: `gh pr ready <pr>`, then
  hand to `/land-pr` — it owns the polling, the false greens and the merge
  rules. One of those rules matters at claim time: a **single-commit** PR
  squashes to the *commit* subject, so the `wip: claim …` subject must never
  be the only commit on a PR being merged.

## Releasing a claim

- **Abandoning:** close the draft PR and comment on the issue that the claim
  is released. Never leave a claim marked with no open PR behind it.
- **A dead claim found in the way** (open draft PR, no activity, no live
  session): ask before taking it over — a dirty worktree somewhere may hold
  unpushed work. Never silently steal a claim.
- One claimed ticket per session. Finish or release before claiming another.

## Final report

```
## claim-ticket: #<n> <title>
**Claimed:** <pr-url> (draft) at <UTC time>
**Selection:** <why this one — priority, what was skipped as claimed>
**Plan:** in the PR body — <one-line summary>
**State:** implementing | blocked on <what> | released because <why>
```
