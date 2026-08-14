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
consumes: ["DesignDoc.md::s2-1", "scripts/lint_spec.py::main", "scripts/lint_spec.py --part {a,b}"]
owns: ["pyproject.toml", "uv.lock", "Makefile", "src/er/__init__.py", "src/er/py.typed", "src/er/config/__init__.py", "src/er/lake/__init__.py", "src/er/ingest/__init__.py", "src/er/matching/__init__.py", "src/er/entities/__init__.py", "src/er/golden/__init__.py", "src/er/review/__init__.py", "src/er/eval/__init__.py", "src/er/embeddings/__init__.py", "scripts/actionlint.py", "tests/unit/test_package_layout.py", "tests/unit/test_spec_lint.py", "configs/.gitkeep", "fixtures/static/.gitkeep", "fixtures/generator/.gitkeep", "benchmarks/baselines/.gitkeep", "models/.gitkeep", "artifacts/.gitkeep", "scripts/ci/.gitkeep", "tests/integration/.gitkeep", "tests/helpers/.gitkeep"]
protected_paths: ["DesignDoc.md", "scripts/lint_spec.py", "scripts/board.py", "scripts/gates.sh"]
extra_paths: [".gitignore"]
attempts: 1
verify: "uv sync --frozen && uv run ruff check . && uv run ruff format --check . && uv run mypy --strict src/er && uv run pytest tests/unit/test_package_layout.py tests/unit/test_spec_lint.py -q"
branch: "ticket/ER-003-repo-skeleton-pyproject-uv-lock-full"
commit: ""
spec_sha: "2e60a757351fe5ce"
updated_at: "2026-08-14T21:19:09Z"
session: 7104a6b7-c393-4d4d-8f16-5c44089825c0
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
- [ ] AC5: Every path `docker/Dockerfile` will COPY — `src/`, `dbt/`, `configs/`, `benchmarks/`, `fixtures/`, `tests/`, `scripts/` — exists in a fresh clone: for each, `git ls-files <dir>` returns at least one entry.
- [ ] AC6: `uv run pytest tests/unit/test_spec_lint.py -q` passes, with one case per linter arm: `--part a`/`--part b` exit 0 against DesignDoc.md and exit 1 against `tests/fixtures/designdoc_v1.0.md`, and the no-`--part` invocation exits 0.
- [ ] AC7: Each Makefile recipe is byte-equal to the command `bash scripts/gates.sh --list` prints for the gate of the same name (asserted by test_package_layout), and `uv run python scripts/actionlint.py --version` prints the S2.1 actionlint version without any network access.

## Tests

- tests/unit/test_package_layout.py::test_uv_lock_matches_s2_1_pins
- tests/unit/test_package_layout.py::test_every_src_subpackage_is_importable
- tests/unit/test_package_layout.py::test_s3_directories_exist_in_git
- tests/unit/test_package_layout.py::test_makefile_recipes_match_gates_sh
- tests/unit/test_spec_lint.py::test_part_a_passes_on_designdoc
- tests/unit/test_spec_lint.py::test_part_a_fails_on_v1_0_fixture
- tests/unit/test_spec_lint.py::test_part_b_passes_on_designdoc
- tests/unit/test_spec_lint.py::test_part_b_fails_on_v1_0_fixture

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
