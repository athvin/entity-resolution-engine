# Evals — claim-ticket

Hand-run. Pose each in a **fresh** session; the signal that matters most is
whether this skill is reached at all, and whether an adjacent one is reached
instead.

This skill carries `disable-model-invocation: true`, so scenarios 1–3 are posed
as `/claim-ticket …`. Scenarios 4 and 5 are the opposite test: they must **not**
pull this skill in, and are posed as plain requests.

## 1. The double claim — two agents pick the same "most important" card

> `/claim-ticket` — (the top-priority In-progress issue already has an open
> draft PR saying `Closes #<n>`, created twenty minutes ago by another session)

**Must:** run the claim check per candidate before claiming — the
`closedByPullRequestsReferences` query, not the board's `Status`; skip the
claimed card and take the next unclaimed one; after creating its own draft PR,
re-run the claim check and yield to the earlier `createdAt` if a race is found.

**Must not:** treat `Status: In progress` as "free to take" or as "someone is
on it" — it is the queue, not the claim; open a second PR against a claimed
issue and keep it; steal an existing claim because it looks idle.

This is the scenario the skill exists for: without a visible claim, two agents
duplicate a ticket's work and collide in review.

## 2. Claim visibility before any code exists

> `/claim-ticket 722`

**Must:** verify #722 is unclaimed, then make all three marks — assignee, an
open **draft** PR carried by an **empty commit** with `Closes #722` in the
body, and a claim comment with a UTC timestamp; cut the worktree from a
freshly pulled `origin/main` per the CLAUDE.md ritual; read the issue and the
files it names, then write the plan into the **PR body** before implementing;
capture the URL `gh pr create` printed rather than predicting a number.

**Must not:** edit any board field (`Done` closes the issue via the
automation; `In progress` is already set); wait for real code before pushing
the draft; chase the draft's first CI run (an empty diff classifies as
`code=true`, so it is a full gate proving a tree identical to `main`); put
the plan only in the session transcript.

## 3. Nothing unclaimed in the queue

> `/claim-ticket` — (every In-progress card has an open linked PR)

**Must:** report that every queued card is claimed, with the PR each claim
points at, and stop.

**Must not:** silently promote a `Todo` card to take it; steal the
least-recently-active claim; set any Status.

## 4. Negative routing — a board question, not a claim

> "Move #640's card into the current iteration and set its priority."

**Must:** answer from `project-board` — field ids derived fresh, one field per
`item-edit`, read-after-write staleness.

**Must not:** reach for `claim-ticket`, which is user-invoked and owns the
claim procedure, not board edits.

## 5. Negative routing — the other end of the lifecycle

> `/land-pr 731` — (the PR is a former claim, now ready and green)

**Must:** run `land-pr`'s loop — required contexts per head SHA, the squash
rules, the main loop.

**Must not:** pull in `claim-ticket`; its job ended when the PR left draft.
The one rule both skills carry is the squash-subject trap: a single-commit PR
squashes to the commit subject, so a bare `wip: claim …` commit must never be
the only commit at merge time.
