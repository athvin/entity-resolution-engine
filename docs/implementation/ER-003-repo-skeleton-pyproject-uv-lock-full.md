---
id: ER-003
title: "Repo skeleton: pyproject + uv.lock (full pinned runtime set), ruff, mypy --strict src/er, package tree, Makefile, pinned actionlint installer"
milestone: M1
status: in_progress
kind: code
size: M
gates: fast
depends_on: ["ER-001"]
spec_refs: ["s2", "s2-1", "s3", "s8-1", "s9-1"]
gap_refs: ["M4", "M25"]
provides: ["pyproject.toml", "uv.lock", "Makefile", "src/er/__init__.py::__version__", "src/er/py.typed", "src/er/config/__init__.py", "src/er/lake/__init__.py", "src/er/ingest/__init__.py", "src/er/matching/__init__.py", "src/er/entities/__init__.py", "src/er/golden/__init__.py", "src/er/review/__init__.py", "src/er/eval/__init__.py", "src/er/embeddings/__init__.py", "scripts/actionlint.py::main", "tests/unit/test_package_layout.py", "tests/unit/test_spec_lint.py", "dir:configs/", "dir:fixtures/static/", "dir:fixtures/generator/", "dir:benchmarks/baselines/", "dir:models/", "dir:artifacts/", "dir:scripts/ci/", "dir:tests/unit/", "dir:tests/integration/", "dir:tests/helpers/"]
consumes: ["DesignDoc.md::s2-1", "scripts/lint_spec.py::main"]
owns: ["pyproject.toml", "uv.lock", "Makefile", "src/er/__init__.py", "src/er/py.typed", "src/er/config/__init__.py", "src/er/lake/__init__.py", "src/er/ingest/__init__.py", "src/er/matching/__init__.py", "src/er/entities/__init__.py", "src/er/golden/__init__.py", "src/er/review/__init__.py", "src/er/eval/__init__.py", "src/er/embeddings/__init__.py", "scripts/actionlint.py", "tests/unit/test_package_layout.py", "tests/unit/test_spec_lint.py", "configs/.gitkeep", "fixtures/static/.gitkeep", "fixtures/generator/.gitkeep", "benchmarks/baselines/.gitkeep", "models/.gitkeep", "artifacts/.gitkeep", "scripts/ci/.gitkeep", "tests/integration/.gitkeep", "tests/helpers/.gitkeep"]
protected_paths: ["DesignDoc.md", "scripts/lint_spec.py", "scripts/board.py", "scripts/gates.sh"]
extra_paths: [".gitignore"]
attempts: 2
verify: "uv sync --frozen && uv run ruff check . && uv run ruff format --check . && uv run mypy --strict src/er && uv run pytest tests/unit/test_package_layout.py tests/unit/test_spec_lint.py -q"
branch: "ticket/ER-003-repo-skeleton-pyproject-uv-lock-full"
commit: ""
spec_sha: "2e60a757351fe5ce"
updated_at: "2026-08-14T22:29:44Z"
session: 15919bcc-f1e1-40c6-ba91-865b2fa305d1
---
## Description

Create the Python project every later gate runs inside: `pyproject.toml` carrying the complete S2.1 runtime and dev pin set, a committed `uv.lock`, ruff and mypy configuration, the `src/er` package tree of S3, the top-level directories `docker/Dockerfile` will COPY, a Makefile whose recipes are exactly the strings `scripts/gates.sh --list` prints, and `scripts/actionlint.py` which runs the binary shipped by the pinned `actionlint-py` wheel and downloads nothing. Until this lands there is no `uv` environment, so no ticket after it can run `uv run` anything. It also wires the spec linter into pytest so the M0 amendment is re-checked on the unit layer.

## Scope

### In scope

- `pyproject.toml`: `requires-python = ">=3.12,<3.13"`, hatchling/uv build of the `er` package from `src/`, every S2.1 pin as an exact `==` requirement, dev group with pytest/pytest-xdist/hypothesis/ruff/mypy/actionlint-py, `[tool.ruff]` and `[tool.mypy]` sections
- `uv.lock` produced by the pinned `uv==0.11.3` and committed
- `src/er/` package tree with `__init__.py` per S3 subpackage and `py.typed`
- Top-level directory skeleton with `.gitkeep`, plus `.gitignore` negations so `artifacts/.gitkeep` and `models/.gitkeep` are tracked while their contents stay ignored
- `Makefile` targets `lint`, `types`, `unit`, `dbt`, `gates`, `spec`, `board`
- `scripts/actionlint.py` — locate and exec the wheel-shipped `actionlint` binary, forward argv, propagate exit status
- `tests/unit/test_package_layout.py` and `tests/unit/test_spec_lint.py`

### Out of scope

- Any module body: `versions.py` (ER-004), `columns.py` (ER-006), `config/` (ER-011), `ids.py` (ER-013), `cli.py`/`errors.py` (ER-014) — this ticket ships empty `__init__.py` files only
- `scripts/ci/actionlint.sh` (ER-010 — the shell wrapper CI calls) and `.github/workflows/` (ER-010)
- The dbt project (ER-008), the Dockerfile (ER-007), compose (ER-009)
- Editing DesignDoc.md or scripts/lint_spec.py to make a parity assertion pass — both are protected

## Design decisions applied

Implements M25 (nothing is pinned) and M4 (the mypy contradiction: the gate is `mypy --strict src/er`, there is no `core/`). Three constraints. (1) S2.1's last rule — adding a dependency means adding a row — makes `pyproject.toml` a closed set: every distribution in the default dependency group MUST have an S2.1 row, so a later ticket that needs `boto3`/`psycopg` needs a spec amendment first, not a quiet lockfile edit. (2) `scripts/gates.sh` already defines the canonical gate command strings and is protected; the Makefile restates them and a test asserts equality, so the two cannot drift. (3) `scripts/actionlint.py` MUST NOT download a binary (S2.1's actionlint row): a linter fetched at CI time is an unpinned dependency guarding a pinning rule.

## Acceptance criteria

- [ ] AC1: `uv sync --frozen` succeeds on a clean checkout and `uv.lock` is committed; `uv run python -c "import er"` exits 0.
- [ ] AC2: For every S2.1 row whose *Asserted by* cell names `uv.lock`, `tests/unit/test_package_layout.py` parses that row out of DesignDoc.md and asserts `uv.lock` resolves that distribution to that exact version — covering splink 4.0.16, duckdb 1.5.5, dbt-core 1.12.2, dbt-duckdb 1.11.0, dbt-adapters 1.24.5, dbt-common 1.39.0, ruff 0.16.3, mypy 2.3.0, pytest 9.1.1, pytest-xdist 3.8.0, hypothesis 6.165.7, actionlint-py 1.7.7.23, typer 0.27.1, pydantic 2.13.4, python-ulid 4.0.1; a version bump in either place with no matching edit in the other fails the test.
- [ ] AC3: `uv run mypy --strict src/er` exits 0, `src/er/py.typed` is committed, and every directory under `src/er/` named in S3 contains an `__init__.py` (asserted by test_package_layout).
- [ ] AC4: `uv run ruff check .` and `uv run ruff format --check .` both exit 0, and `[tool.ruff]` pins `target-version = "py312"`.
- [ ] AC5: Every path `docker/Dockerfile` will COPY that this ticket owns — `src/`, `configs/`, `benchmarks/`, `fixtures/`, `tests/`, `scripts/` — exists in a fresh clone: for each, `git ls-files <dir>` returns at least one entry. `dbt/` is asserted by ER-008, which owns it.
- [ ] AC6: `uv run pytest tests/unit/test_spec_lint.py -q` passes, with one case per linter arm: `python3 scripts/lint_spec.py DesignDoc.md` exits 0; `python3 scripts/lint_spec.py tests/fixtures/designdoc_v1.0.md` exits 1; `python3 scripts/lint_spec.py --expect-fail tests/fixtures/designdoc_v1.0.md` exits 0; and `python3 scripts/lint_spec.py --expect-fail DesignDoc.md` exits 1. The last two arms are what prove the linter is not vacuous.
- [ ] AC7: Each Makefile recipe is byte-equal to the command `bash scripts/gates.sh --list` prints for the gate of the same name (asserted by test_package_layout), and `uv run python scripts/actionlint.py --version` prints the S2.1 actionlint version without any network access.

## Tests

- tests/unit/test_package_layout.py::test_uv_lock_matches_s2_1_pins
- tests/unit/test_package_layout.py::test_every_src_subpackage_is_importable
- tests/unit/test_package_layout.py::test_s3_directories_exist_in_git
- tests/unit/test_package_layout.py::test_makefile_recipes_match_gates_sh
- tests/unit/test_spec_lint.py::test_lint_passes_on_designdoc
- tests/unit/test_spec_lint.py::test_lint_fails_on_v1_0_fixture
- tests/unit/test_spec_lint.py::test_expect_fail_passes_on_v1_0_fixture
- tests/unit/test_spec_lint.py::test_expect_fail_fails_on_designdoc

## Verification

```bash
uv sync --frozen && uv run ruff check . && uv run ruff format --check . && uv run mypy --strict src/er && uv run pytest tests/unit/test_package_layout.py tests/unit/test_spec_lint.py -q
uv run python scripts/actionlint.py --version
bash scripts/gates.sh --fast
```

## Definition of Done

- Acceptance criteria met
- Verify command passes
- `uv.lock` committed and produced by uv 0.11.3
- Every default dependency has an S2.1 row; no undeclared dependency added
- `.gitignore` updated so `artifacts/.gitkeep` and `models/.gitkeep` are tracked
- `scripts/actionlint.py` contains no download (`curl`/`wget`/`urlopen`)
- DesignDoc.md, lint_spec.py, board.py and gates.sh unmodified
- Committed on main

## Blocker log

### Attempt 1 — underspecified (2026-08-14T21:21:17Z)

- **Failing command:** `python3 scripts/lint_spec.py --part a DesignDoc.md   # and: git ls-files dbt`
- **Assertion / contradiction:** Two acceptance criteria are unsatisfiable by construction, so no plan-check-valid plan for ER-003 exists.

(1) AC6 requires "--part a/--part b exit 0 against DesignDoc.md and exit 1 against tests/fixtures/designdoc_v1.0.md", and names four tests (test_part_a_passes_on_designdoc, test_part_a_fails_on_v1_0_fixture, test_part_b_passes_on_designdoc, test_part_b_fails_on_v1_0_fixture). But scripts/lint_spec.py has no --part flag: "lint_spec.py: error: unrecognized arguments: --part DesignDoc.md" (exit 2). Its only arms are the bare path and --expect-fail. ER-001, status done, lists "scripts/lint_spec.py --part {a,b}", "scripts/lint_spec.py::PART_A_SECTIONS" and "scripts/lint_spec.py::PART_A_TOKENS" in provides; grep -c for all three in scripts/lint_spec.py returns 0. ER-001 was seeded done against an unmet provides contract. ER-003 cannot repair it: scripts/lint_spec.py is in ER-003 protected_paths, and ER-003 Scope/Out of scope says "Editing DesignDoc.md or scripts/lint_spec.py to make a parity assertion pass - both are protected".

(2) AC5 requires that for each of src/, dbt/, configs/, benchmarks/, fixtures/, tests/, scripts/, "git ls-files <dir> returns at least one entry". git ls-files dbt returns 0 entries. ER-003 owns no path under dbt/; ER-008 (status todo) owns dbt/dbt_project.yml, dbt/profiles/profiles.yml, dbt/models/.gitkeep and the rest, and ER-003 Out of scope defers "The dbt project (ER-008)". plan-check rejects a plan touching a path another unfinished ticket owns, so AC5 cannot be covered from inside ER-003.

The spec is not at fault. DesignDoc.md S2.1, S3, S8.1 and S9.1 are all consistent and implementable; these are two ticket-authoring defects.
- **Smallest change that would unblock:** Two edits to ticket text, no code and no spec change.

(1) AC6: drop --part entirely and assert the two arms the linter actually has. Replace AC6 with: "uv run pytest tests/unit/test_spec_lint.py -q passes, with one case per linter arm: python3 scripts/lint_spec.py DesignDoc.md exits 0, python3 scripts/lint_spec.py tests/fixtures/designdoc_v1.0.md exits 1, and python3 scripts/lint_spec.py --expect-fail tests/fixtures/designdoc_v1.0.md exits 0 while the same flag against DesignDoc.md exits 1." Rename the four Tests node ids to test_lint_passes_on_designdoc, test_lint_fails_on_v1_0_fixture, test_expect_fail_passes_on_v1_0_fixture, test_expect_fail_fails_on_designdoc. Then remove "scripts/lint_spec.py --part {a,b}", "::PART_A_SECTIONS" and "::PART_A_TOKENS" from ER-001 provides, and "scripts/lint_spec.py --part {a,b}" from ER-002 and ER-003 consumes. Nothing is lost: the non-vacuity property the --part split was meant to guarantee is already enforced by --expect-fail, which CI runs on the committed v1.0 copy. The alternative - reopening ER-001 to actually build a PART_A/PART_B split - is strictly more work for the same guarantee.

(2) AC5: delete dbt/ from the AC5 path list in ER-003, leaving src/, configs/, benchmarks/, fixtures/, tests/, scripts/. Add the dbt/ arm to ER-008 instead, as a new acceptance criterion: "git ls-files dbt returns at least one entry, so docker/Dockerfile can COPY it." ER-008 owns dbt/ and ER-007 (the Dockerfile) depends on it, so the assertion lands on the ticket that can satisfy it.

After both edits, re-run python3 scripts/board.py validate, then python3 scripts/board.py unblock ER-003.

Worth a wider pass while you are in there: this is a provides-vs-reality drift, and ER-001/ER-002 were seeded done in Phase A rather than built by the loop, so their other provides entries were never executed either. Checking every ER-001/ER-002 provides symbol against scripts/lint_spec.py and DesignDoc.md now would catch the rest of this class before it stops another ticket.
- **Log:** `.loop/logs/ER-003.attempt-1.log`

### Attempt 2 — environment (2026-08-14T22:29:01Z)

- **Failing command:** `uv --version`
- **Assertion / contradiction:** Permission to use Bash has been denied because Claude Code is running in don't ask mode. Every uv invocation is refused by the harness before the process starts (tried 'uv --version', 'uv version', 'command -v uv'), while git, python3 scripts/board.py and bash scripts/gates.sh are permitted in the same session. ER-003's verify command is five consecutive uv invocations and 6 of its 7 acceptance criteria are uv invocations, so none of them can be executed. AC2 additionally requires a uv.lock produced by uv 0.11.3, a full transitive resolution that cannot be hand-authored without fabricating the artifact its own test exists to catch.
- **Smallest change that would unblock:** Add "Bash(uv:*)" to permissions.allow in .claude/settings.json (or settings.local.json) and re-run the loop; the first 'uv lock'/'uv sync' also needs network egress to pypi.org. No repo, board or spec change is required - attempt 1's two ticket defects were already fixed in f547709 and the ticket is now implementable.
- **Log:** `.loop/logs/ER-003.attempt-2.log`
