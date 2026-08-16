---
id: ER-062
title: "assertions: table + er assert add|remove|load + assertions.csv format (symbolic assertion_id) + precedence + check_contradiction_1 (unit)"
milestone: M3
status: in_progress
kind: code
size: M
gates: full
depends_on: ["ER-013", "ER-014", "ER-019", "ER-021"]
spec_refs: ["s4-4", "s4-4-1", "s4-0", "s4-7", "s5", "s5-0", "s8-2-1", "s12-1"]
gap_refs: ["M6", "M1", "M19"]
provides: ["src/er/review/__init__.py", "src/er/review/assertions.py::Assertion", "src/er/review/assertions.py::AssertionInput", "src/er/review/assertions.py::add_assertion", "src/er/review/assertions.py::retract_assertion", "src/er/review/assertions.py::active_assertions", "src/er/review/assertions.py::assertion_id_for", "src/er/review/assertions.py::parse_assertions_csv", "src/er/review/assertions.py::load_assertions_csv", "src/er/review/assertions.py::check_contradiction_1", "src/er/review/assertions.py::AssertionConflict", "src/er/review/assertions.py::Contradiction", "cli:er assert add", "cli:er assert remove", "cli:er assert load", "relation:assertions"]
consumes: ["src/er/entities/ids.py::record_key", "src/er/entities/ids.py::canonicalize_pair", "src/er/entities/ids.py::UlidFactory", "src/er/errors.py::ConfigError", "src/er/errors.py::StageFailure", "src/er/cli.py::app", "src/er/config/schema.py::Config", "src/er/lake/ducklake.py::connect", "src/er/lake/ddl.py::apply", "src/er/lake/model.py::TABLE_SPECS", "src/er/obs/run_context.py::RunContext", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env", "relation:assertions"]
owns: ["src/er/review/assertions.py", "tests/unit/review/__init__.py", "tests/unit/review/test_assertions_model.py", "tests/integration/test_assertions_cli.py"]
protected_paths: []
extra_paths: ["src/er/cli.py", "src/er/review/__init__.py"]
attempts: 1
verify: "uv run pytest tests/unit/review/test_assertions_model.py -q && bash scripts/ci/itest.sh tests/integration/test_assertions_cli.py -q"
branch: "ticket/ER-062-assertions-table-er-assert-add-remove"
commit: ""
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-16T17:28:46Z"
session: 1d026dff-c729-494f-84fe-3f432a87eae6
---
## Description

S4.4 makes assertions the mechanism by which steward corrections survive re-runs, and S5 gives the relation a full lifecycle (`assertion_id`, `active`, `retracted_by`/`retracted_at`) so the assertion delta between runs is computable. Today the table exists from `er init` but nothing writes it, no verb retracts, precedence is unenforced, and CONTRADICTION-1 (S4.4.1) has no implementation. This ticket ships `src/er/review/assertions.py`, the `er assert add|remove|load` verbs of S4.0, the `assertions.csv` input format of S8.2.1, write-time precedence enforcement, and `check_contradiction_1` as a pure function over an active assertion set.

## Scope

### In scope

- `src/er/review/assertions.py`: canonical-pair insert, retraction (never delete), active-set reader, and `assertion_id_for(conn, rec_a_key, rec_b_key)` symbolic resolution
- `er assert add --a KEY --b KEY --kind always|never --by USER [--note TEXT]`, `er assert remove --assertion-id ID --by USER`, `er assert load --path FILE` with the S4.0 stdout line and exit codes
- `parse_assertions_csv(path)` for the S8.2.1 literal header `phase,rec_a_key,rec_b_key,kind,created_by,note`, returning rows carrying their `phase` from the `{base, batch, refresh, resurrect}` vocabulary
- Write-time precedence: `never` dominates `always` for the same canonical pair; a conflicting insert is rejected (exit `1`) rather than ordered around
- `check_contradiction_1(assertions) -> list[Contradiction]`: connected components of the active `always` edges, returning every active `never` pair inside one component with the offending `assertion_id`s and the closure component

### Out of scope

- Wiring CONTRADICTION-1 into `er reconcile` as a hard pre-clustering failure — that is ER-074, which owns the exit path and the no-snapshot guarantee
- Edge adjustment / injection of `always` edges into the clustering edge set (ER-070) and the partition-level `never` cut (ER-076)
- `review_queue` and the resolution → assertion transaction (ER-063)
- The affected-node assertion delta (ER-069)
- Authoring any scenario `assertions.csv` fixture (ER-077, ER-078)

## Design decisions applied

Implements gap-report M6 (lifecycle + precedence + CONTRADICTION-1), M1 (canonical pair ordering) and M19 (the missing CLI verbs), under D5 and D9. Constraints that are easy to miss: (1) rows are NEVER deleted — `remove` sets `active=false` and stamps `retracted_by`/`retracted_at`, because the assertion delta between runs is computed from those stamps (S4.5.1); (2) every write goes through the single canonicalisation helper in `ids.py` so `rec_a_key < rec_b_key` holds on every row (S5.0) — a caller passing `--a` greater than `--b` gets a canonicalised row, not an error; (3) the S8.2.1 header is LITERAL and this ticket must not add a column: the symbolic `assertion_id` requirement is satisfied by referring to assertions by canonical pair, never by ULID, with `assertion_id_for` doing the resolution — that is what keeps fixture files stable across runs; (4) `er assert load` applies every row in the file (a per-phase slice is the caller's job) and exits `10` when every row is already present and active; (5) precedence is enforced at write time only — `check_contradiction_1` is a pure function taking a sequence of assertions and returns findings, it never queries or fails a run itself.

## Acceptance criteria

- [ ] AC1: `er assert add --a webforms:9 --b crm:1 --kind always --by tester` exits `0`, writes exactly one row with `rec_a_key < rec_b_key` (the arguments are canonicalised, not reflected verbatim), `active=true`, `created_by='tester'`, and prints `assertion_id, rec_a_key, rec_b_key, kind, active`
- [ ] AC2: Adding a `never` for a pair that already has an active `always` (and the reverse) exits `1`, leaves exactly one active row for that pair, and writes no second row; the rejection message names the existing `assertion_id`
- [ ] AC3: `er assert add` with a key containing a second `':'` or with `--kind maybe` exits `2` and writes zero rows
- [ ] AC4: `er assert remove --assertion-id <id> --by tester` exits `0`, sets `active=false` with `retracted_by`/`retracted_at` populated, leaves the row count unchanged, and a subsequent `add` of the same pair with the opposite `kind` now succeeds
- [ ] AC5: `er assert load --path <csv>` over a file with the literal S8.2.1 header applies every row and prints one line per row; re-running the same file exits `10` and writes zero new rows
- [ ] AC6: `parse_assertions_csv` rejects a file whose header is not byte-equal to `phase,rec_a_key,rec_b_key,kind,created_by,note`, and rejects a `phase` value outside `{base, batch, refresh, resurrect}`
- [ ] AC7: `check_contradiction_1` over `always(a,b)`, `always(b,c)`, `never(a,c)` returns one `Contradiction` carrying all three `assertion_id`s and the component `{a,b,c}`; over the same set with the `never` retracted it returns `[]`; a `never` whose endpoints are in two different always-components returns `[]`
- [ ] AC8: `dbt test --select tag:keys` stays green after the fixture inserts: at most one `active` row per `(rec_a_key, rec_b_key)` and every row satisfies `rec_a_key < rec_b_key`

## Tests

- tests/unit/review/test_assertions_model.py::test_canonicalises_pair_on_write
- tests/unit/review/test_assertions_model.py::test_never_dominates_always_conflict_rejected
- tests/unit/review/test_assertions_model.py::test_parse_assertions_csv_header_and_phase_vocabulary
- tests/unit/review/test_assertions_model.py::test_check_contradiction_1_finds_always_closure_violation
- tests/unit/review/test_assertions_model.py::test_check_contradiction_1_clean_set_returns_empty
- tests/integration/test_assertions_cli.py::test_assert_add_remove_lifecycle
- tests/integration/test_assertions_cli.py::test_assert_add_exit_codes
- tests/integration/test_assertions_cli.py::test_assert_load_is_idempotent

## Verification

```bash
uv run pytest tests/unit/review/test_assertions_model.py -q
bash scripts/ci/itest.sh tests/integration/test_assertions_cli.py -q
uv run mypy --strict src/er/review
```

## Definition of Done

- `er assert` appears in `er --help` with exactly the S4.0 flag set; no flag beyond that table is added
- No code path deletes an `assertions` row
- `check_contradiction_1` is importable and unit-tested without a lake connection
- Canonicalisation goes through `ids.canonicalize_pair` — no second ordering implementation exists in `src/er/review/`
- `bash scripts/gates.sh` green; INTERFACES entry lists the module's public symbols
