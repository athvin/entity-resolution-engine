---
id: ER-103
title: "Docs: README, docs/runbook.md, CONTRIBUTING + lint_docs.py (links, CLI coverage, exit-code sections, S8.3 node-id resolution)"
milestone: M5
status: todo
kind: docs
size: M
gates: fast
depends_on: ["ER-024", "ER-025", "ER-035", "ER-094", "ER-099", "ER-101"]
spec_refs: ["s3", "s4-0", "s4-7", "s5-1", "s6-1", "s8-1", "s8-3", "s9-1", "s9-2", "s10-3", "s12"]
gap_refs: ["M19", "M22", "M23", "M24", "MINOR-milestones"]
provides: ["README.md", "docs/runbook.md", "CONTRIBUTING.md", "scripts/lint_docs.py::main", "scripts/lint_docs.py::check_links", "scripts/lint_docs.py::check_cli_coverage", "scripts/lint_docs.py::check_exit_code_sections", "scripts/lint_docs.py::check_s8_3_node_ids", "tests/unit/test_docs.py"]
consumes: ["DesignDoc.md", "docs/implementation/BOARD.md", "scripts/gates.sh", "scripts/ci/itest.sh", "benchmarks/report.py", ".github/workflows/ci.yaml", ".github/workflows/benchmark.yaml", "src/er/cli.py", "src/er/errors.py", "tests/integration/test_invariants.py"]
owns: ["README.md", "CONTRIBUTING.md", "docs/runbook.md", "scripts/lint_docs.py", "tests/unit/test_docs.py"]
protected_paths: ["DesignDoc.md", ".github/workflows/ci.yaml", ".github/workflows/benchmark.yaml"]
extra_paths: []
attempts: 0
verify: "uv run pytest tests/unit/test_docs.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Write the three human-facing documents the repository has never had — `README.md` (what the platform is, the quickstart chain, the S3 repo map), `docs/runbook.md` (the operator surface: every `er` command from S4.0 with its flags and exit codes, the S4.7 error taxonomy and `--resume`, lake maintenance, the correction pass, S5.1 version-bump rebuilds, and benchmark dispatch/baseline operations) and `CONTRIBUTING.md` (uv/frozen-lock workflow, the gate ladder, the board contract, forbidden suppressions) — and gate them with `scripts/lint_docs.py`. The lint exists because prose about a CLI drifts silently: it checks link resolution, CLI coverage in both directions against S4.0, one documented section per exit code, and that every S8.3 pytest node id resolves to a real test function. Closes M19's 'the CLI is the orchestration contract but is not specified' at the documentation layer, M22's node-id requirement, and the M23/M24 operator gaps.

## Scope

### In scope

- `README.md`: purpose, the S3 layout map, the quickstart (`docker compose … up` substrate → `er init` → `er doctor` → `er run-all`), pointers to the runbook, CONTRIBUTING, `DesignDoc.md` and the board.
- `docs/runbook.md`: one section per S4.0 command (flags with defaults, required env from S6.1, exit codes, stdout shape); one section per exit code `0/1/2/3/10` with the S4.0 meaning and the operator action; the S4.7 error-class table with retryable-vs-terminal and `--resume`; `er lake maintain` retention and the single-writer lock; `er correct` cadence and what it restores; S5.1 breaking-change and version-bump rebuild procedure; benchmark dispatch, `--write-baseline` bootstrap and the S10.3 verdict table; the CI job map (S9.1/S9.2) and where artifacts land.
- `CONTRIBUTING.md`: `uv sync --frozen`, the gate ladder and scopes, ticket/board workflow, forbidden suppressions, the two dbt targets, and the rule that adding a dependency means adding an S2.1 row.
- `scripts/lint_docs.py` with the four named checks, exiting non-zero with a `file:line: message` per finding.
- `tests/unit/test_docs.py` invoking the lint on the committed tree plus one negative arm per check over a temporary copy.

### Out of scope

- Editing `DesignDoc.md` or restating spec normative content instead of citing it — the spec is protected here and remains the single specification.
- Adding a `Docs lint` step to `ci.yaml`: S9.1 owns the static job's step list; this lint runs inside the unit job via `tests/unit/test_docs.py`.
- Generating docs from code (no autodoc), and any published/site build.
- Duplicating the S8.3 table or the S4.0 exit-code table verbatim into the runbook — cite anchors and document the operator action.
- Running pytest, Docker or the lake from `lint_docs.py`; node-id resolution is a static AST parse.

## Design decisions applied

Closes M19/M22/M23/M24 at the docs layer plus the MINOR-milestones item. Constraints an implementer will otherwise miss: (1) the docs lint must be hermetic — no `pytest --collect-only`, no Docker, no network — so S8.3 node ids are resolved by parsing each named file's AST for the named `def`; T-INV-1's node id `tests/integration/test_invariants.py::test_membership_equals_connected_components` is reported by an autouse finalizer, so that file must define the reporting function and the lint asserts it rather than special-casing it away. (2) CLI coverage is checked in **both** directions: every command and flag in S4.0's table appears in the runbook, and every `er <verb>`/flag appearing in any fenced block of the three documents exists in S4.0 — the second direction is what catches invented verbs. (3) Exit code `10` is 'nothing to do' and never aborts an `er run-all` chain, while `2` and `3` do; the runbook must say so, because that distinction is the whole S4.1.1 empty-delivery guard. (4) `S9.1` owns the CI step list, so this lint is wired through the unit layer, not a new workflow step. (5) Documented commands must be the `uv run …` / `bash scripts/ci/itest.sh` forms the repo actually uses; a bare `python` invocation is a defect the lint may flag.

## Acceptance criteria

- [ ] AC1: `uv run python scripts/lint_docs.py` exits 0 on the committed tree and prints no findings.
- [ ] AC2: On a temporary copy with the `er lake maintain` section removed from `docs/runbook.md`, the lint exits non-zero and its output names `er lake maintain`; the same holds for any single S4.0 command removed.
- [ ] AC3: On a temporary copy in which the runbook documents a flag absent from S4.0 (e.g. `er ingest --dry-run`), the lint exits non-zero naming that flag and the file:line.
- [ ] AC4: On a temporary copy containing a relative link to a nonexistent file and a `DesignDoc.md#s99` anchor link, the lint reports both with their file:line and exits non-zero.
- [ ] AC5: On a temporary copy in which `tests/integration/test_invariants.py` is deleted, the lint exits non-zero naming the T-INV-1 node id; with every S8.3 node id resolvable it exits 0.
- [ ] AC6: `docs/runbook.md` contains a distinct section for each of exit codes `0`, `1`, `2`, `3`, `10`, and contains all seven S4.7 `error_class` literals (`transient_io`, `lock_conflict`, `precondition`, `config`, `contradiction`, `non_convergence`, `data`) and the five S4.0 exit-`3` precondition failures.
- [ ] AC7: `README.md`'s quickstart block contains only commands whose `er` verbs and flags exist in S4.0, and every path it references exists in the repository.
- [ ] AC8: `scripts/lint_docs.py` runs with no network, no Docker and no lake: `tests/unit/test_docs.py` passes under the fast gate scope and completes without spawning `pytest` or `docker`.

## Tests

- tests/unit/test_docs.py::test_lint_docs_clean_on_committed_tree
- tests/unit/test_docs.py::test_missing_cli_section_is_reported
- tests/unit/test_docs.py::test_undocumented_flag_is_reported
- tests/unit/test_docs.py::test_broken_file_and_anchor_links_are_reported
- tests/unit/test_docs.py::test_unresolvable_s8_3_node_id_is_reported
- tests/unit/test_docs.py::test_runbook_covers_every_exit_code_and_error_class
- tests/unit/test_docs.py::test_readme_quickstart_commands_and_paths_exist

## Verification

```bash
uv run pytest tests/unit/test_docs.py -q
uv run python scripts/lint_docs.py
uv run ruff check . && uv run ruff format --check .
bash scripts/gates.sh --ticket ER-103
```

## Definition of Done

- `README.md`, `docs/runbook.md`, `CONTRIBUTING.md` committed; every S4.0 command and flag documented; every exit code has its own section.
- `scripts/lint_docs.py` implements the four checks and reports `file:line: message`; each check has a negative arm in `tests/unit/test_docs.py`.
- Every S8.3 pytest node id resolves to a defined test function, including the T-INV-1 reporting shim.
- No step added to `ci.yaml`; `DesignDoc.md` unmodified.
- Lint is hermetic (no pytest subprocess, no Docker, no network).
- `scripts/gates.sh --ticket ER-103` green with a receipt.
