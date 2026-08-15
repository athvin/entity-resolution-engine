---
name: implementing-er-tickets
description: Implements exactly one READY ticket from the Entity Resolution board in docs/implementation, end to end - selects the next dependency-unblocked ticket, reads the DesignDoc.md sections it cites, writes the code and tests, runs the ticket's verify command and the repo gate ladder, then commits and marks it done, or blocks it with a written note. Use when asked to work the ER board, pick up the next ER ticket, implement ER-NNN, or continue the ER build.
argument-hint: "[ER-NNN]"
disable-model-invocation: true
effort: high
allowed-tools: >-
  Bash(python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py *)
  Bash(bash ${CLAUDE_PROJECT_DIR}/scripts/gates.sh *)
  Bash(python3 scripts/board.py *) Bash(bash scripts/gates.sh *) Bash(./scripts/gates.sh *)
  Bash(uv run pytest *) Bash(uv run ruff *) Bash(uv run mypy *) Bash(uv run dbt *)
  Bash(uv run python *) Bash(uv sync *) Bash(uv lock *) Bash(uv add *)
  Bash(uv --version) Bash(command -v *)
  Bash(docker compose *) Bash(docker build *) Bash(docker info)
  Bash(git status *) Bash(git diff *) Bash(git log *) Bash(git show *) Bash(git rev-parse *)
  Bash(git switch *) Bash(git add *) Bash(git commit *) Bash(git merge --ff-only *) Bash(git branch *)
  Bash(mkdir -p *)
  Read Write Edit Grep Glob WebFetch
disallowed-tools: >-
  Bash(git push *) Bash(git reset --hard *) Bash(git rebase *) Bash(git clean *)
  Bash(git restore *) Bash(git checkout *) Bash(git branch -D *) Bash(gh *)
  Bash(bash scripts/run-loop.sh *) Bash(scripts/run-loop.sh *)
  Edit(docs/implementation/**) Edit(.loop/receipts/**)
  AskUserQuestion
---

# Implementing one ER ticket

You implement **exactly one ticket, then stop**. A driver re-invokes you with a fresh context for the
next one, so nothing you learn here carries forward — everything durable goes on disk.

Requested ticket (empty means "pick the next one"): $ARGUMENTS

## Standing rules

These apply for the whole task, not just when you first read them.

1. **`board.py` owns every status change.** Never hand-edit a ticket's YAML frontmatter. If a
   transition is refused, the refusal is correct — read it and act on it.
2. **Run the ticket's `verify` command verbatim.** Never narrow it, rewrite it, or substitute a
   cheaper command. If it is wrong, that is a `spec_contradiction` block, not an edit.
3. **Never make a gate pass by suppressing it.** No `# type: ignore`, `# noqa`, `skip`, `xfail`,
   `pragma: no cover`, no loosening a lint config, no deleting or weakening an assertion, no editing a
   test to match wrong output. `gates.sh` greps your diff for these and fails.
4. **Never edit `DesignDoc.md`, `.claude/**`, or `scripts/{board,gates,run-loop}`.** If the spec is
   wrong, block with class `spec_contradiction` and write the wording you propose.
5. **Stop after one ticket.** Do not start a second, even if the first was quick.
6. **Report honestly.** A blocked ticket with a good note is a useful outcome. A ticket marked done
   that is not done poisons every ticket that depends on it.

## The iteration

Copy this checklist into your reply and tick items off as you go.

```
Iteration:
- [ ] 0. Preflight
- [ ] 1. Select
- [ ] 2. Claim + branch
- [ ] 3. Read ticket + cited spec
- [ ] 4. Plan, then validate the plan
- [ ] 5. Implement
- [ ] 6. Ticket verify passes
- [ ] 7. Gate ladder passes
- [ ] 8. Commit -> complete -> merge
- [ ] 9. Report, then STOP
```

### 0. Preflight

```bash
python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py validate
git status --porcelain
git rev-parse --abbrev-ref HEAD
```

Board invalid → report `STATUS=board_invalid` with the defect lines and stop; do not repair it by
editing files. Tree dirty, or not on `main` → report `STATUS=dirty_tree` and stop.

### 1. Select

If the requested ticket above is non-empty, use it. Otherwise:

```bash
python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py next --json
```

Exit 10 → report `STATUS=no_ready_tickets` and stop. **This is the loop's normal termination.**
Exit 1 → another ticket is already `in_progress`; report `STATUS=board_busy` naming it, and stop.

### 2. Claim and branch

```bash
python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py claim <ID> --session ${CLAUDE_SESSION_ID}
git switch -c ticket/<ID>-<slug>
```

`claim` increments `attempts` **before** any work, so a crash still costs an attempt and a doomed
ticket cannot spin forever. It commits its own board write. If it refuses, report the reason verbatim
and stop — never work an unclaimed ticket.

**If `git switch -c` fails because the branch already exists, stop.** Block with class
`environment`, naming the branch. A leftover branch is an abandoned earlier attempt whose tip is
*behind* the `claim` commit `board.py` just wrote to `main`. Switching to it puts you on stale code,
and everything you build there will fail `merge --ff-only` at step 8 after the work is done. Never
`git switch` to an existing ticket branch, and never rebase or force it into shape.

### 3. Read

- The **whole** ticket file, including any `## Blocker log`. A previous attempt's note is the most
  valuable context you have; do not repeat its approach.
- **Every section named in `spec_refs`**, from `DesignDoc.md`. Read them. Do not recall them.
- `docs/implementation/INTERFACES.md` — what earlier tickets already built. Reuse it rather than
  writing a second helper that does the same thing.
- The code and tests the ticket touches.

### 4. Plan, then validate it

Write `.loop/change-plan.json`:

```json
{
  "ticket": "ER-013",
  "files": [{"path": "src/er/entities/ids.py", "action": "create",
             "why": "record_key + pair canonicalisation", "covers": ["AC1", "AC2"]}],
  "tests": ["tests/unit/test_ids.py"],
  "gates_scope": "fast",
  "risks": []
}
```

```bash
python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py plan-check .loop/change-plan.json --ticket <ID>
```

**Write no code until this exits 0.** It rejects a plan that leaves an acceptance criterion
uncovered, names no test file, touches a `protected_paths` entry, or touches a path another
unfinished ticket owns. Revise and re-run; after 3 failed cycles the ticket is under-specified →
block with class `underspecified`.

### 5. Implement

Only paths the validated plan names. If you discover you need another file, update
`.loop/change-plan.json` and re-run `plan-check` — the plan must stay true, because `gates.sh` fails
if the diff strays outside it.

Write the tests the ticket names. Match the conventions of the surrounding code; see
[references/conventions.md](references/conventions.md).

### 6. Ticket verify

```bash
python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py verify-cmd <ID>   # prints the command
```

Run that command **exactly**. On failure: read the output, fix the code, run it again. After 3 failed
cycles, block with class `verify_failed`.

### 7. Gate ladder

```bash
bash ${CLAUDE_PROJECT_DIR}/scripts/gates.sh --ticket <ID>
```

This runs hygiene → spec lint → board lint → ruff → mypy → unit → dbt, plus the Compose integration
suite when the change warrants it, then re-runs the ticket verify and writes a **receipt** binding the
result to the exact tree it tested. On failure, fix and re-run the **whole ladder** — a fix can break
an earlier gate. After 3 failed cycles, block with class `gate_failed`.

A full-scope run can take fifteen minutes or more. **Run it in the foreground with
`timeout: 3600000` and wait for it to finish.** You are a headless single-shot session: if you end
your turn while a gate is still running, the session ends, the gate dies unrecorded, and the ticket is
left `in_progress` having consumed an attempt. There is no "pick it up when it finishes".
What each gate proves: [references/gates.md](references/gates.md).

### 8. Commit, complete, merge

Only when steps 6 and 7 both exited 0 in this session, with their output visible above.

```bash
git add -- <exactly the paths in your validated plan, and nothing else>
git commit -m "<ID> <ticket title>

<1-3 lines: what changed and why, in spec terms>

Spec: DesignDoc.md S<a>, S<b>
Verify: <the verify command>
Gates: <scope + list from the gates.sh output>
Ticket: docs/implementation/<file>

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"

python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py complete <ID> --commit $(git rev-parse HEAD)
git switch main && git merge --ff-only ticket/<ID>-<slug>
```

Never `git add -A` — it sweeps up build output, caches and anything you wrote outside the plan. Do
not stage the ticket file either: `board.py` writes and commits every frontmatter transition itself,
and an unplanned change under `docs/implementation/` fails the hygiene gate.

`complete` re-checks the commit, requires a gate receipt whose tree equals the committed tree, and
independently re-runs the verify for fast-scope tickets. **A refusal means you are not actually
done.** Fix it or block; do not argue with it.

If `merge --ff-only` fails, `main` moved under you: block with class `environment`. Do not rebase.

### 9. Report

Emit exactly this block, then stop:

```
TICKET: ER-NNN — <title>
STATUS: done | blocked | no_ready_tickets | board_invalid | board_busy | dirty_tree
FILES:  <paths changed>
VERIFY: <command> -> <exit code>
GATES:  <scope> -> <pass/fail per gate>
COMMIT: <sha or ->
BOARD:  <done>/<total> done, <ready> ready, <blocked> blocked
NEXT:   <id of the next READY ticket, or ->
```

## When something goes wrong

1. Capture the failing command and its output to `.loop/logs/<ID>.attempt-<K>.log`.
2. If there is partial work worth keeping, commit it on the ticket branch as
   `wip(<ID>): blocked — <class>`. Never merge it to `main`. Never silently discard it.
3. Block, with a note a human can act on without re-deriving your session:

```bash
python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py block <ID> \
  --class <verify_failed|gate_failed|underspecified|spec_contradiction|environment> \
  --failing-command "<the exact command>" \
  --assertion "<the exact assertion, error, or contradiction>" \
  --smallest-fix "<the smallest change that would unblock this>" \
  --log .loop/logs/<ID>.attempt-<K>.log
```

4. Report `STATUS=blocked` and stop. `board.py` returns you to `main` itself.

**Blocked means excluded from READY until a human runs `board.py unblock`** — so a block is never a
silent skip, but it does cost the board a ticket. Use it when you are genuinely stuck, not to avoid
hard work.

`environment` is special: it returns the ticket to `todo` and refunds the attempt, because a missing
Docker daemon is not the ticket's fault. Use it only for genuine environment failures — a denied tool
call, an unavailable daemon, a moved `main` — never for a test you could not make pass.

Choosing the right class, and the quality bar for a blocker note:
[references/blocking.md](references/blocking.md).

## Reference

- [references/board-format.md](references/board-format.md) — the ticket contract and the full `board.py` CLI
- [references/gates.md](references/gates.md) — what each gate proves, and the forbidden-suppression list
- [references/blocking.md](references/blocking.md) — failure taxonomy and blocker-note quality bar
- [references/conventions.md](references/conventions.md) — where code goes and how it is invoked
