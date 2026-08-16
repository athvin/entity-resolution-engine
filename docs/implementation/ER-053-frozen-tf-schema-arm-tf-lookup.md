---
id: ER-053
title: "Frozen-TF schema arm: tf_lookup, tf_snapshot_id/tf_tables_path, materialize_tf_lookup, register_tf, compute_tf_table confinement guard"
milestone: M3
status: done
kind: code
size: M
gates: full
depends_on: ["ER-019", "ER-048", "ER-049"]
spec_refs: ["s4-3-3", "s4-3-1", "s5", "s5-0", "s6", "s12-1", "s4-0b"]
gap_refs: ["M9", "D4"]
provides: ["src/er/matching/tf.py::tf_columns", "src/er/matching/tf.py::new_tf_snapshot_id", "src/er/matching/tf.py::materialize_tf_lookup", "src/er/matching/tf.py::register_tf", "src/er/matching/tf.py::tf_tables_path", "src/er/matching/tf.py::parse_tf_tables_path", "src/er/matching/tf.py::MissingTfLookupError", "relation:tf_lookup"]
consumes: ["src/er/lake/ddl.py::apply", "src/er/lake/model.py::TABLE_SPECS", "src/er/matching/model.py::build_settings", "src/er/matching/splink_env.py::splink_api", "src/er/matching/splink_env.py::assert_no_splink_relations_in_lake", "src/er/entities/ids.py::new_ulid", "src/er/config/schema.py::ErConfig", "tests/conftest.py::lake_ns"]
owns: ["src/er/matching/tf.py", "tests/unit/matching/test_tf_registration.py", "tests/integration/test_tf_schema.py"]
protected_paths: []
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/matching/test_tf_registration.py -q && bash scripts/ci/itest.sh tests/integration/test_tf_schema.py -q"
branch: "ticket/ER-053-frozen-tf-schema-arm-tf-lookup"
commit: "dde7502395bb13a8df7d2b9fe126d47823b11e6b"
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-16T12:14:53Z"
session: bdf3070a-d7b1-46ae-b25c-821e5fa30e42
---
## Description

Implements the schema and plumbing half of the frozen term-frequency policy of S4.3.3 (D4): `tf_lookup` materialization keyed by `(model_version, tf_snapshot_id)`, the `tf_snapshot_id` mint, the `tf_tables_path` locator written onto `model_registry`, and `register_term_frequency_lookup` wiring for every `tf: true` column. Without this, Splink recomputes TF from whatever corpus is registered at predict time and INV-SCORE (S4.3.3) is false — the same pair scores differently in a base run, an incremental run and a full re-resolution. This ticket ships the mechanism and the confinement guard; the behavioural corpus-size-independence arm is ER-061.

## Scope

### In scope

- `tf_columns(cfg)` — the ordered list of `comparisons` keys with `tf: true`
- `new_tf_snapshot_id()` — a ULID minted through `ids.py`, the only mint site
- `materialize_tf_lookup(conn, cfg, model_version, tf_snapshot_id)` — writes one row per `(column_name, value)` into `lake.main.tf_lookup`, computed from `lake.main.int_std_records` over non-null values, and returns the row count
- `register_tf(linker, conn, model_version, tf_snapshot_id)` — one `linker.table_management.register_term_frequency_lookup(...)` call per TF column, reading the frozen rows; raises `MissingTfLookupError` when a TF column has zero rows for the key
- `tf_tables_path(model_version, tf_snapshot_id)` / `parse_tf_tables_path(s)` — the single producer/parser pair for the `model_registry.tf_tables_path` string, pinned as `lake.main.tf_lookup?model_version=<mv>&tf_snapshot_id=<ts>`
- The `compute_tf_table` confinement guard: a source scan asserting `materialize_tf_lookup` is the only call site in the repository

### Out of scope

- `er train` itself, `model_registry` row writing, `model_version` allocation (ER-054/ER-055)
- Scoring, `match_scores` writes, gray-band handling (ER-058)
- Rebuilding TF under a new snapshot from `er correct` (ER-094) — this ticket only exposes the mint and the materializer that path will call
- The T-TF-1 flip bound (deliberately deferred per the board)
- Corpus-size independence of `match_probability` (ER-061)

## Design decisions applied

Implements D4 (LOCKED, S12.1) and closes M9. Three constraints are easy to miss: (1) outside `er train` nothing may mint a `tf_snapshot_id` except `er correct` via `er match --mode full --new-tf-snapshot`, so `new_tf_snapshot_id` must be called from exactly those two paths and the guard test enforces it; (2) `linker.table_management.compute_tf_table(...)` may be called from nowhere but `materialize_tf_lookup`, which itself is invoked only by `er train` — an incremental or full scoring run that computes TF silently breaks INV-SCORE; (3) `tf: true` applies only to exact-match levels on the named column (S4.3.1), so `tf_columns` reads `comparisons.<attr>.tf` and nothing else. `tf_lookup`'s logical key `(model_version, tf_snapshot_id, column_name, value)` is unenforced by DuckLake (S5.0) — the materializer must not rely on the engine to reject a duplicate. Import consumed symbols from INTERFACES.md; where a path differs, INTERFACES.md wins.

## Acceptance criteria

- [ ] AC1: After `materialize_tf_lookup` over `base_10`, `select distinct column_name from lake.main.tf_lookup` equals `tf_columns(cfg)` = `{given_name, family_name, email}`, and columns without `tf: true` (`phone_e164`, `birth_date`, `addr_postal`) contribute zero rows
- [ ] AC2: For a chosen column and value, `tf_value` equals that value's relative frequency among non-null values of the column in `int_std_records`, within 1e-12
- [ ] AC3: `select count(*) - count(distinct (model_version, tf_snapshot_id, column_name, value))` over `tf_lookup` returns 0 after two consecutive materializations under the same key (re-materializing replaces rather than appends)
- [ ] AC4: `register_tf` invokes `register_term_frequency_lookup` exactly once per TF column, in `tf_columns` order, and never invokes `compute_tf_table` — asserted against a spy linker recording call names and kwargs
- [ ] AC5: `register_tf` against a `(model_version, tf_snapshot_id)` with no `tf_lookup` rows raises `MissingTfLookupError` naming the column, and the CLI path surfacing it exits 3 rather than scoring with computed TF
- [ ] AC6: `parse_tf_tables_path(tf_tables_path(mv, ts)) == (mv, ts)` for a ULID `ts` and a zero-padded `mv`; a malformed string raises
- [ ] AC7: A source scan of `src/er/`, `fixtures/` and `tests/` finds `compute_tf_table` only inside `src/er/matching/tf.py::materialize_tf_lookup`, and `new_tf_snapshot_id` only in `train.py`, `full.py`'s `--new-tf-snapshot` branch and `tf.py` itself
- [ ] AC8: After a materialize + register + one Splink call sequence, `assert_no_splink_relations_in_lake(conn)` passes and the `tf_lookup` row count is unchanged

## Tests

- tests/unit/matching/test_tf_registration.py::test_register_tf_calls_register_term_frequency_lookup_per_column
- tests/unit/matching/test_tf_registration.py::test_compute_tf_table_has_one_call_site
- tests/unit/matching/test_tf_registration.py::test_tf_snapshot_id_mint_is_confined
- tests/unit/matching/test_tf_registration.py::test_tf_tables_path_round_trips
- tests/unit/matching/test_tf_registration.py::test_tf_columns_reads_only_tf_true
- tests/integration/test_tf_schema.py::test_materialize_writes_one_row_per_value
- tests/integration/test_tf_schema.py::test_tf_values_equal_corpus_relative_frequency
- tests/integration/test_tf_schema.py::test_rematerialize_does_not_duplicate_the_key
- tests/integration/test_tf_schema.py::test_missing_tf_lookup_is_a_precondition_failure

## Verification

```bash
uv run pytest tests/unit/matching/test_tf_registration.py -q && bash scripts/ci/itest.sh tests/integration/test_tf_schema.py -q
uv run mypy --strict src/er/matching/tf.py
```

## Definition of Done

- Acceptance criteria met and the verify command passes
- `tf_lookup` is created by `ddl.py` from the existing TableSpec registry — no new `CREATE TABLE` text in this ticket
- `compute_tf_table` confinement guard is a test, not a comment
- `tf_tables_path` format documented in the module docstring and produced/parsed in exactly one place
- `mypy --strict src/er/matching/tf.py` clean
- `provides` entries recorded in INTERFACES.md
- Committed on a branch off main
