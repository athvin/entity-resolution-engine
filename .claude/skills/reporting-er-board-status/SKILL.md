---
name: reporting-er-board-status
description: Reports the state of the Entity Resolution implementation board in docs/implementation - how many tickets are done, what is ready next, what is blocked and why, and which milestone the build is in. Use when asked how the ER build is going, what the next ticket is, why a ticket is blocked, what is left to do, or for a progress summary of the entity resolution engine.
allowed-tools: >-
  Bash(python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py *)
  Bash(git log *) Read Grep Glob
disallowed-tools: >-
  Bash(bash scripts/run-loop.sh *) Bash(scripts/run-loop.sh *)
  Write Edit
---

# Reporting on the ER board

Read-only. Never claim, complete, block or unblock a ticket from this skill — those are the worker
skill's job, and running them here would corrupt an in-flight iteration.

## Gather

```bash
python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py status --json
python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py next --json
```

`next` exits 10 when nothing is READY. Read its payload before concluding anything: a board with
zero READY tickets and zero blocked is **complete**; one with blocked or starved tickets is
**stalled**, which is a very different answer.

For a blocked ticket, read its `## Blocker log` — the last entry names the failing command, the
assertion, and the smallest proposed fix:

```bash
python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py show <ID>
```

For recent progress, `git log --oneline -20 -- docs/implementation` shows the board transitions and
`git log --oneline -20` the work commits, whose subjects all start with a ticket id.

## Report

Lead with the number that answers the question, then the detail. Something like:

> **37 of 104 done** — in M2 (standardize), 9 of 16 complete.
> **Next up:** ER-047, `int_blocking_keys` macro-generated from the config payload.
> **Blocked (1):** ER-041 — `base_10` fixture. The committed CSV headers do not match
> `sources.crm.columns` in `configs/test.yaml`; the note proposes renaming the fixture columns to
> the config's names. Needs a human to pick which side moves.

Adapt the shape to what was asked. If someone asks only "what's next", one line is the right answer.

Two things to be careful about:

- **Do not infer progress from `done` alone.** A board can be 60% done and stalled because the one
  blocked ticket gates everything remaining. If anything is blocked, say so and say what it gates —
  `unblocks` in `next --json` is the fan-out count.
- **Do not speculate about why a ticket failed.** Quote its blocker note. It was written by the agent
  that hit the failure and is more reliable than a reconstruction from the diff.
