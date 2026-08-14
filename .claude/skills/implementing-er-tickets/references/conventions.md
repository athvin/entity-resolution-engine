# Working conventions

## Contents
- Where things live
- Invocation
- Code style
- Tests
- Recording what you learned

## Where things live

`DesignDoc.md` S3 is the authority for the repository layout; read it rather than guessing, and if
this file and the spec ever disagree, the spec wins.

The two facts that are local to this loop and not in the spec:

- `.loop/` holds everything the loop generates — change plans, gate receipts, logs, per-run
  transcripts, the blocked log. It is gitignored. Never commit it.
- `docs/implementation/` holds the board. `board.py` writes it; you do not.

## Invocation

Always invoke the loop scripts with these exact forms. Permission rules match the literal command
string, so a relative path does not match an absolute rule and the call is denied.

```bash
python3 ${CLAUDE_PROJECT_DIR}/scripts/board.py <subcommand>
bash    ${CLAUDE_PROJECT_DIR}/scripts/gates.sh --ticket <ID>
```

Project commands run through `uv`, never a bare `python`:

```bash
uv run pytest tests/unit/test_x.py -q
uv run mypy --strict src/er
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
```

A denied tool call is an environment blocker, not something to work around with a different command
spelling. Block with class `environment` and name the denied call.

## Code style

Match the surrounding code — its naming, its comment density, its idioms. The repo is `ruff`-formatted
and `mypy --strict`, so:

- Type every signature in `src/er/`. `--strict` means no implicit `Any`, and `cast(Any, …)` to escape
  it is a suppression.
- Comment *why*, not *what*. A comment explaining a non-obvious constraint (a DuckLake limitation, a
  determinism requirement, an ordering that is load-bearing) earns its place; one narrating the next
  line does not.
- Prefer a named constant with a one-line justification over a bare magic number.

## Tests

Every code ticket names its tests, and the plan must list the test files. Unit tests must not need
Docker; anything requiring the lake belongs in `tests/integration/` and pushes the ticket to full
gate scope.

Write the test so it fails before your change. If you cannot make it fail first, you have not
established that it tests anything.

## Recording what you learned

Your context is discarded when this iteration ends. Two things survive, and both matter to the next
ticket:

- **`provides:`** on the ticket already lists what this ticket creates; `board.py complete`
  regenerates `docs/implementation/INTERFACES.md` from it. Read that file in step 3 so you reuse
  earlier work instead of writing a second implementation of it.
- If you made a convention decision another ticket will need to follow — an error-message format, a
  fixture layout, a naming rule the spec did not pin — say so in your commit message body. It is the
  only durable place the next context will look.
