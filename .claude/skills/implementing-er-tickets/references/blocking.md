# Blocking: when, which class, and what to write

## Contents
- The cost of a block
- Choosing a class
- The blocker-note quality bar
- Why you may not edit the spec
- What happens next

## The cost of a block

A blocked ticket is removed from READY until a human runs `board.py unblock`. Everything that depends
on it becomes unreachable too. On a board where one foundational ticket unblocks thirty others, a
careless block stops the build.

So: block when you are genuinely stuck after using your three attempts at the failing step. Do not
block to avoid hard work, and do not block because a task is tedious.

The opposite failure is worse. Marking a ticket done when it is not poisons every ticket downstream,
and the poison is discovered much later, in a test whose failure points somewhere else entirely.
**A well-written block is a good outcome. A false completion is not.**

## Choosing a class

| Class | Use when | Effect |
|---|---|---|
| `verify_failed` | The ticket's own verify command still fails after 3 fix cycles, and you believe the ticket is implementable. | blocked |
| `gate_failed` | A repo gate still fails after 3 cycles — a type error you cannot resolve, an integration test that will not pass. | blocked |
| `underspecified` | The ticket is too thin to implement: `plan-check` cannot be satisfied, an acceptance criterion is unfalsifiable, or a required decision is absent. The spec is fine; the ticket is not. | blocked |
| `spec_contradiction` | Two statements in `DesignDoc.md` cannot both hold, or the spec contradicts the ticket. | blocked |
| `environment` | The machine is the problem: Docker not running, a network fetch failing, a tool absent, a denied tool call, `main` moved under you. | **returns to `todo`, refunds the attempt** |

`environment` is deliberately forgiving because an absent Docker daemon is not the ticket's fault —
without the refund, one missing daemon would permanently block every integration ticket on the board
in about twenty minutes. That forgiveness is also why it must not be abused: a test you could not
make pass is `verify_failed`, not `environment`.

## The blocker-note quality bar

`board.py block` requires four fields and rejects anything under twelve characters, because a note a
human cannot act on is worse than no note. Write for someone who has none of your context.

- `--failing-command` — the exact command, copy-pasteable, not a paraphrase.
- `--assertion` — the exact assertion, error message, or the two spec sentences that conflict, quoted.
- `--smallest-fix` — the smallest change you believe would unblock it. This is the field that matters;
  it is the difference between a human spending five minutes and an hour.
- `--log` — the captured output at `.loop/logs/<ID>.attempt-<K>.log`.

Bad: `--assertion "tests failed"`.
Good: `--assertion "tests/unit/test_ids.py::test_canonical_pair_order asserts canonicalize_pair('b:1','a:2') == ('a:2','b:1'), but S5.0 orders on the full record_key string, which puts 'a:2' first only if compared lexically — the ticket's example in AC3 assumes numeric ordering of the suffix. AC3 and S5.0 disagree."`

## Why you may not edit the spec

You are graded against `DesignDoc.md`. An agent that can edit its own acceptance criteria is not being
verified by anything. So the spec, this skill, and the loop scripts are all outside your write access,
enforced by permission rules and re-checked by the hygiene gate and by the driver after every
iteration.

When the spec is wrong, the correct move is to block with `spec_contradiction` **and write the exact
replacement wording you propose** in `--smallest-fix`. A human applies it, and the tickets that cite
that section become implementable. That path is fast when your note is precise.

## What happens next

`board.py block` appends your note to the ticket's `## Blocker log`, records a line in
`.loop/blocked.log`, returns you to `main`, and commits the board change. Any partial work you
committed stays on the ticket branch.

The next attempt at this ticket — after a human unblocks it — starts with a fresh context and reads
your note first. Write it for that reader.
