---
id: ER-005
title: "scripts/lint_board.py: frontmatter schema, id↔filename, acyclic depends_on, enum + non-empty verify/spec_refs, milestone↔S12 parity"
milestone: M1
status: todo
kind: code
size: S
gates: fast
depends_on: ["ER-003"]
spec_refs: ["s3", "s5", "s5-0", "s8-3", "s9-1", "s12"]
gap_refs: ["MINOR-milestones", "M22"]
provides: ["scripts/lint_board.py::lint_board", "scripts/lint_board.py::main", "scripts/lint_board.py::FRONTMATTER_SCHEMA", "tests/unit/test_board_lint.py"]
consumes: ["scripts/board.py", "DesignDoc.md::s8-3", "DesignDoc.md::s12", "DesignDoc.md::s5-0", "pyproject.toml"]
owns: ["scripts/lint_board.py", "tests/unit/test_board_lint.py"]
protected_paths: ["DesignDoc.md", "scripts/board.py"]
extra_paths: ["docs/implementation/README.md"]
attempts: 0
verify: "uv run pytest tests/unit/test_board_lint.py -q && python3 scripts/lint_board.py docs/implementation"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:39Z"
---
## Description

`scripts/lint_board.py` is the static-job gate that keeps the ticket board machine-consistent and keeps it honest against the spec: it validates each ticket's frontmatter against the schema `scripts/board.py` already enforces at runtime, checks `id`↔filename, rejects duplicate ids and dependency cycles, requires a non-empty `verify` and `spec_refs` that resolve to real `<a id=…>` anchors, and — per S9.1 — fails when any S5 relation or any S8.3 test id is unassigned to a milestone in S12, or when a milestone cites an id that does not exist. Without it MINOR-milestones recurs silently: an S8.3 arm can be split off and then never gated by any milestone.

## Scope

### In scope

- YAML frontmatter parse + schema check for every `docs/implementation/ER-*.md`
- Enum checks reusing `scripts/board.py`'s STATUSES / SIZES / KINDS / GATES rather than re-declaring them
- `id` uniqueness, `id`↔filename prefix agreement, non-empty `verify`, non-empty `spec_refs`
- `spec_refs` anchor resolution against DesignDoc.md's `<a id=…>` set
- `depends_on` closure: every referenced id exists; the graph is acyclic (report the cycle)
- S12 parity: every S8.3 test id and every S5 relation name appears in at least one milestone row; every id a milestone cites exists in S8.3 / S5. NOT "exactly one": a milestone cell may name an arm another milestone owns (M1 says "T-KEY-1a and not T-KEY-1b"), so four ids legitimately appear twice. The invariant that matters is that no arm is forgotten
- Default path `docs/implementation` when invoked with no argument, so the S9.1 static step `lint_board.py` works unmodified
- Removing the four synthetic eval leftovers from `docs/implementation` (the evals scaffold their own board into throwaway repos — `tests/skill-evals/run-evals.sh` writes `ER-001-first.md` itself), and noting the directory's contract in `docs/implementation/README.md`

### Out of scope

- Modifying `scripts/board.py` — it is the runtime writer and is protected here; the linter reads its constants
- Rewriting BOARD.md or authoring/reordering tickets
- Adding milestone assignments to S12 (a spec change — ER-002 owns S12)
- Checking ticket bodies for prose quality; only the machine-checkable frontmatter and the S12 parity are in scope

## Design decisions applied

Implements MINOR-milestones (M1's exit criterion and the orphan relations) and the M22 consequence that every S8.3 row must be reachable from a milestone. Three constraints. (1) The enums MUST be imported from `scripts/board.py`, not copied — two enum lists drift and the board writer wins at runtime. (2) `docs/implementation` currently holds four leftover synthetic tickets, two of which share the id `ER-001`; the verify command runs the linter over that directory, so they must be deleted for it to exit 0 — deleting them is safe because `run-evals.sh` generates its fixtures into a temp repo. (3) `spec_refs` anchor resolution is what makes a ticket's citations verifiable; an anchor that does not exist in DesignDoc.md is a defect even when the section number looks plausible.

## Acceptance criteria

- [ ] AC1: `python3 scripts/lint_board.py docs/implementation` exits 0 on the committed board, and `python3 scripts/lint_board.py` with no argument lints the same directory and exits 0.
- [ ] AC2: Two ticket files carrying the same `id` exit 1 with a message naming both filenames. The duplicate-id case is built as a synthetic fixture in a temporary directory, not by editing the real board.
- [ ] AC3: A ticket whose filename does not start with its `id`, or whose `status`/`kind`/`gates`/`size` is outside the corresponding tuple in `scripts/board.py`, or whose `verify` or `spec_refs` is empty, exits 1 naming the file and the field.
- [ ] AC4: A `spec_refs` entry that is not an `<a id=…>` anchor present in DesignDoc.md exits 1 naming the ticket and the unresolved anchor (e.g. `s99-9`).
- [ ] AC5: A `depends_on` id with no ticket file exits 1; an injected cycle `A → B → A` exits 1 printing the cycle in order.
- [ ] AC6: `python3 scripts/lint_board.py docs/implementation` exits 0 against the committed `DesignDoc.md`. On a temp copy with `T-BLK-1` removed from every S12 milestone cell, the linter exits 1 naming `T-BLK-1` as unassigned; on a copy whose M2 cell cites `T-NOPE-9`, it exits 1 naming the unknown id. A test id appearing in two milestone cells is NOT a defect.
- [ ] AC7: Exit codes are exactly 0 (clean), 1 (defects, one `DEFECT` line each on stderr), 2 (usage or unreadable directory).
- [ ] AC8: `uv run mypy --strict scripts/lint_board.py` is not required, but `uv run ruff check scripts/lint_board.py` exits 0 and the module imports nothing outside the stdlib and `scripts/board.py`.

## Tests

- tests/unit/test_board_lint.py::test_committed_board_is_clean
- tests/unit/test_board_lint.py::test_default_path_is_docs_implementation
- tests/unit/test_board_lint.py::test_duplicate_id_fails
- tests/unit/test_board_lint.py::test_filename_must_start_with_id
- tests/unit/test_board_lint.py::test_enum_and_required_field_violations_fail
- tests/unit/test_board_lint.py::test_unresolved_spec_anchor_fails
- tests/unit/test_board_lint.py::test_dependency_cycle_is_reported
- tests/unit/test_board_lint.py::test_s8_3_id_unassigned_in_s12_fails

## Verification

```bash
uv run pytest tests/unit/test_board_lint.py -q && python3 scripts/lint_board.py docs/implementation
python3 scripts/lint_board.py
python3 scripts/board.py validate --strict
```

## Definition of Done

- Acceptance criteria met
- Verify command passes
- Enums sourced from scripts/board.py, not duplicated
- Leftover synthetic tickets removed from docs/implementation; skill-eval E3 still green
- `spec_refs` anchor resolution and S12 parity both proven by a failing-fixture test
- DesignDoc.md and scripts/board.py unmodified
- Committed on main
