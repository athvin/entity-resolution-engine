# The gate ladder

## Contents
- What each gate proves
- Scope
- Receipts
- Forbidden suppressions
- Running the slow gate

## What each gate proves

`bash ${CLAUDE_PROJECT_DIR}/scripts/gates.sh --list` prints the exact command for each. CI asserts
this table matches that output.

| Gate | Proves |
|---|---|
| hygiene | The change introduces no suppression, touches no `protected_paths`, does not modify the loop's own machinery or the spec, and stays inside the validated plan. |
| spec | `DesignDoc.md` still contains every section, anchor and term the tickets cite — nobody has quietly re-litigated the design. |
| board | Every ticket still parses, the dependency graph is still acyclic, every `spec_refs` anchor still resolves. |
| lint | `ruff check` and `ruff format --check` — style is mechanical, so it is never a judgement call. |
| types | `mypy --strict src/er` — the strictness is the point; weakening it is a suppression. |
| unit | The pure-function layer: normalizers, reconciler, ids, config validators, comparison helpers. Fast enough to run on every fix cycle. |
| dbt | `dbt parse` against the `mem` target — models compile without a warehouse. |
| integration | The Compose substrate: real DuckLake, real Postgres catalog, real object store, the scenario tests. This is the only gate that proves the pipeline works. |

## Scope

`fast` = everything except integration. `full` = everything.

The ticket's `gates:` field sets the floor. Touching `src/er/`, `dbt/`, `docker/`, `fixtures/` or
`tests/integration/` escalates to `full` regardless of what the ticket declares — a ticket cannot opt
out of the gate that would catch it.

Scope is computed from the **working tree** (unstaged + staged + untracked), not from a
`main...HEAD` diff. Gates run before the commit, so a three-dot diff would be empty at exactly the
moment it matters.

Several gates run tooling the board itself builds: `integration` needs `scripts/ci/itest.sh`, `dbt`
needs a dbt project, and `lint`/`types`/`unit` all run through `uv` and so need `pyproject.toml`.
Until those land the gate cannot run, so `gates.sh` records it as `-2` — **skipped, not passed** —
and says so loudly.

Two things keep that from becoming a hole. `board.py complete` refuses a `-2` from the ticket whose
own `owns` list contains that tooling, so the ticket that builds the integration runner cannot skip
proving it works. And deleting `scripts/ci/itest.sh` fails the hygiene gate. You will see the `-2`
note in `complete`'s output; it is expected early in the board and not something to work around.

## Receipts

`gates.sh` writes `.loop/receipts/<ID>-<attempt>.json` recording the tree it tested
(`git write-tree`), every gate's exit code, and the verify command with its exit code.

`board.py complete` refuses unless a receipt exists whose `tree` equals the **committed** tree. This
is what makes "done" mean something: run gates, then edit code, then commit, and the trees no longer
match, so the completion is refused. Do not attempt to write or edit a receipt — it is the one
artifact you must not author.

## Forbidden suppressions

The hygiene gate greps your added lines for these and fails:

`# type: ignore` · `# noqa` · `@pytest.mark.skip` · `@pytest.mark.xfail` · `# pragma: no cover` ·
`.skip(` · `pytest.xfail(`

Equally forbidden, and caught by review rather than grep: loosening a `ruff`/`mypy` config, deleting
or weakening an assertion, narrowing a fixture, `cast(Any, …)` to appease `--strict`, adding a
permissive `conftest.py`, or editing a test's expected value to match wrong output.

If a ticket genuinely requires one, it declares `allowed_suppressions` in its own frontmatter. You
cannot grant that to yourself — it is ticket data, set when the ticket was written.

## Running the slow gate

A cold full-scope run builds an image and runs the Compose integration suite; it can take several
minutes and will exceed a foreground shell call. Run it in the background and poll:

```bash
bash ${CLAUDE_PROJECT_DIR}/scripts/gates.sh --ticket <ID>   # with run_in_background: true
```

Results are cached by tree hash, so re-running an unchanged tree is free. Pass `--no-cache` only when
you have a specific reason to distrust the cache.
