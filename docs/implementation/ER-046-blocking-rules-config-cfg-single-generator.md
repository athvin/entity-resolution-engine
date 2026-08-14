---
id: ER-046
title: "blocking_rules_from_config(cfg): the single generator, NULL/empty policy, duplicate/unknown-column rejection"
milestone: M2
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-011", "ER-012"]
spec_refs: ["s4-2", "s6", "s6-1", "s5", "s4-3-4"]
gap_refs: ["M12"]
provides: ["src/er/matching/model.py::blocking_rules_from_config", "src/er/matching/model.py::BlockingKeySpec", "src/er/matching/model.py::BLOCKING_DBT_VAR", "src/er/matching/__init__.py", "tests/unit/matching/test_blocking_generator.py"]
consumes: ["src/er/config/schema.py::Config", "src/er/config/loader.py::load_config", "src/er/errors.py::ErConfigError", "src/er/lake/columns.py::STD_RECORD_COLUMNS", "configs/test.yaml"]
owns: ["src/er/matching/model.py", "tests/unit/matching/test_blocking_generator.py"]
protected_paths: ["src/er/config/schema.py"]
extra_paths: ["src/er/matching/__init__.py"]
attempts: 0
verify: "uv run pytest tests/unit/matching/test_blocking_generator.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Implement `blocking_rules_from_config(cfg) -> tuple[dbt_var_payload, list[BlockingRuleCreator]]` in `src/er/matching/model.py` — the single generator that makes `configs/*.yaml` `blocking:` the one source of truth for both consumers (S4.2). The dbt side receives a serialisable payload from which `int_blocking_keys` is macro-generated; Splink receives `block_on()` over the byte-identical `expr` string, which is the precondition T-BLK-1 checks. The function is pure (config in, payload plus rule objects out, no database), rejects duplicate `key_type`s and expressions referencing columns outside the S5 `int_std_records` column list, and carries the normative NULL/empty policy.

## Scope

### In scope

- `blocking_rules_from_config(cfg)` returning `(payload, rules)` with rule order equal to config order
- `BlockingKeySpec` (`key_type`, `expr`, and the rendered `where` predicate) and the dbt var name the CLI passes
- Duplicate `key_type` and unknown-column rejection with the S6.1 message keys `blocking.duplicate_key_type` / `columns.unknown`, raised as a config error (exit 2)
- The NULL/empty policy: `<expr> is not null and <expr> <> ''`, emitted once here and consumed verbatim by the macro
- Unit tests over `configs/test.yaml` and hand-built configs

### Out of scope

- The `int_blocking_keys` dbt model and macro (ER-047)
- The Splink settings builder / comparison levels (ER-048) — this ticket adds only the blocking-rule list
- T-BLK-1 parity against a real Splink run (ER-057)
- Any DuckDB or lake access; the function must not open a connection

## Design decisions applied

Closes M12 (three descriptions of blocking with no generation direction, no NULL policy, no parity check) by naming one generator and one direction: config → payload → macro, and config → `block_on(expr)`. Easy to miss: the `expr` string handed to Splink must be byte-identical to the one embedded in the dbt branch, so no normalisation, re-quoting or whitespace fixing may happen on either path; rule order is load-bearing because Splink deduplicates candidates via preceding-rule exclusion; the dbt-derived pair set is only comparable after `SELECT DISTINCT` over canonicalised pairs, so the payload must not encourage a multiplicity-sensitive join; `int_blocking_keys` is never an input to scoring (S4.3.4) — this function is the only bridge.

## Acceptance criteria

- [ ] AC1: For `configs/test.yaml`, the payload is a 4-element sequence in config order whose `key_type` values are `['email_exact','phone_exact','name_postal','dob_name']` and whose `expr` strings are byte-identical to the config values, and the returned rule list has 4 entries whose generated SQL each contains its corresponding `expr`.
- [ ] AC2: The payload is JSON-round-trippable and stable: `json.dumps(payload, sort_keys=True)` is byte-identical across two calls on the same config and across a config whose unrelated mapping keys were reordered.
- [ ] AC3: A config with two entries sharing a `key_type` raises the config error carrying `blocking.duplicate_key_type` and names the repeated value.
- [ ] AC4: A config whose `expr` references a column absent from the S5 `int_std_records` column list (e.g. `nickname`) raises the config error carrying `columns.unknown` and names the offending column; an `expr` over `addr_postal`, `family_name`, `email`, `phone_e164`, `birth_date` or `given_name` is accepted.
- [ ] AC5: Each payload entry carries the rendered predicate exactly `<expr> is not null and <expr> <> ''` with `<expr>` substituted, and the test compares it character-for-character against the S4.2 template.
- [ ] AC6: The module opens no database connection and imports no `duckdb` symbol (asserted by AST/import inspection), so the function is callable from a bare unit test.
- [ ] AC7: Adding a fifth `blocking:` entry to a copy of the config adds exactly one payload entry and one rule, in last position, with no other output changing.

## Tests

- tests/unit/matching/test_blocking_generator.py::test_payload_matches_config_order_and_exprs
- tests/unit/matching/test_blocking_generator.py::test_payload_is_stable_and_serialisable
- tests/unit/matching/test_blocking_generator.py::test_duplicate_key_type_rejected
- tests/unit/matching/test_blocking_generator.py::test_unknown_column_rejected
- tests/unit/matching/test_blocking_generator.py::test_null_empty_predicate_is_the_spec_template
- tests/unit/matching/test_blocking_generator.py::test_generator_is_pure_no_duckdb_import
- tests/unit/matching/test_blocking_generator.py::test_added_rule_appends_only

## Verification

```bash
uv run pytest tests/unit/matching/test_blocking_generator.py -q
uv run mypy --strict src/er/matching
uv run ruff check src/er/matching
bash scripts/gates.sh
```

## Definition of Done

- All acceptance criteria met and the verify command passes
- `src/er/matching/model.py` exists with `blocking_rules_from_config` as its only public entry point for this ticket (the settings builder lands in ER-048 in the same module)
- `mypy --strict` clean on `src/er/matching`
- No config schema or validator was modified — rejection paths reuse the existing error taxonomy
- Committed on main with the board updated
