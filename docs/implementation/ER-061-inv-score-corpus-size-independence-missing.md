---
id: ER-061
title: "INV-SCORE: corpus-size independence + missing-tf_lookup precondition failure (TF behavioural arm)"
milestone: M3
status: done
kind: code
size: M
gates: full
depends_on: ["ER-053", "ER-056", "ER-058"]
spec_refs: ["s4-3", "s4-3-3", "s4-3-4", "s4-0", "s4-7", "s5", "s5-0", "s8-1", "s12-1"]
gap_refs: ["M9", "D4"]
provides: ["src/er/matching/tf.py::assert_tf_lookup_complete", "src/er/matching/tf.py::TfLookupIncomplete", "tests/integration/test_tf_policy.py::test_score_is_corpus_size_independent", "tests/integration/test_tf_policy.py::test_missing_tf_lookup_exits_3", "tests/integration/test_tf_policy.py::test_tf_registered_per_column_and_compute_tf_table_never_called"]
consumes: ["src/er/matching/tf.py::materialize_tf_lookup", "src/er/matching/tf.py::register_tf", "src/er/matching/full.py::score_full", "src/er/matching/model.py::build_settings", "src/er/errors.py::PreconditionFailure", "src/er/obs/run_context.py::RunContext", "src/er/lake/ducklake.py::connect", "fixtures/static/model_test_v1.json", "fixtures/static/model_test_v1.meta.json", "fixtures/static/base_10/base", "fixtures/generator/emit.py::emit_corpus", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env", "relation:match_scores", "relation:tf_lookup", "relation:model_registry"]
owns: ["tests/integration/test_tf_policy.py"]
protected_paths: ["fixtures/static/model_test_v1.json", "fixtures/static/base_10/base"]
extra_paths: ["src/er/matching/tf.py", "src/er/matching/full.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_tf_policy.py -q"
branch: "ticket/ER-061-inv-score-corpus-size-independence-missing"
commit: "9870eb44b2de7f1707694f3ff66ce3719d0b6d3d"
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-23T11:41:32Z"
session: c379438e-8c95-4be8-8f5f-3832d8ceafaa
---
## Description

D4 freezes term frequency at training time, and S4.3.3 states INV-SCORE: `match_probability` is a pure function of `(model_version, tf_snapshot_id, rec_a_key, rec_b_key, rec_a_content_hash, rec_b_content_hash)`. ER-053 shipped the `tf_lookup` schema, `materialize_tf_lookup` and `register_tf`; nothing yet proves the frozen values actually make scoring corpus-independent, and nothing stops a run scoring with no registered lookup at all (Splink would silently recompute or drop the adjustment). This ticket adds the behavioural arm: a preflight guard that refuses to score when the frozen lookup is incomplete, and an integration test that scores the same pairs against a small and a ~40x larger corpus under one `tf_snapshot_id` and asserts exactly equal probabilities.

## Scope

### In scope

- `assert_tf_lookup_complete(conn, model_version, tf_snapshot_id, tf_columns)` in `src/er/matching/tf.py`, raising a precondition failure that names every `tf: true` column with zero `tf_lookup` rows
- Wiring that guard into the `er match` preflight for BOTH `--mode full` and `--mode incremental`, before any Splink object is constructed and before any `match_scores` write
- `tests/integration/test_tf_policy.py`: corpus-size independence, missing-lookup refusal, per-column registration, `compute_tf_table` never called outside `er train`
- Asserting the `match_scores` rows written under a grown corpus keep the same `(model_version, tf_snapshot_id)` and the same probability under the S4.3.4 MERGE key

### Out of scope

- T-TF-1 (the bound on how many edges a TF refresh flips across `auto_merge`) — deliberately deferred; it needs the benchmark corpus
- Minting a new `tf_snapshot_id`, `er match --new-tf-snapshot`, and `er correct` (ER-094)
- Retraining, `model_version` allocation, or the mixed-model reconcile guard (ER-085)
- The static `compute_tf_table` confinement guard, which ER-053 already ships
- Any clustering, membership or entity assertion — this ticket reads `match_scores` only

## Design decisions applied

Implements D4 (S12.1) and closes gap-report M9's behavioural half. Constraints an implementer will otherwise miss: (1) the refusal is exit `3` with `error_class='precondition'` (S4.0/S4.7), never `1` — a missing frozen lookup is a precondition, not a scoring failure; (2) the guard must run before any `match_scores` row is written so a refused run is a true no-write; (3) 'bit-equal' means Python `==` on the DOUBLE, with no tolerance — a tolerance here would hide exactly the drift the ticket exists to detect; (4) the two corpora must be scored at the SAME `--model-version` and the SAME registry `tf_snapshot_id`, and the second corpus is grown by ingesting generated records into the same namespace rather than by rebuilding the lake, so the base_10 rows are re-scored through the S4.3.4 MERGE key and the comparison is against persisted prior values; (5) scenario tests never train (S8.3) — load `fixtures/static/model_test_v1.json` and its committed `tf_lookup`.

## Acceptance criteria

- [ ] AC1: After `er match --mode full` over `base_10` and a second `er match --mode full` over `base_10` plus a generated ~1,000-record corpus in the same namespace at the same `--model-version` and `tf_snapshot_id`, every pair present in both runs has exactly equal `match_probability` (compared with `==`, no tolerance), and the compared set includes at least one pair whose winning comparison level is an exact level on a `tf: true` column
- [ ] AC2: After the second run, `match_scores` holds at most one row per `(rec_a_key, rec_b_key, model_version, tf_snapshot_id)`, and every base_10 row still carries its original `rec_a_content_hash`/`rec_b_content_hash` and `is_active = true`
- [ ] AC3: Deleting the `tf_lookup` rows for exactly one `tf: true` column and re-running `er match --mode full` exits `3`, names that `column_name` in `run_stages.error_detail` with `error_class='precondition'`, and adds zero rows to `match_scores`
- [ ] AC4: A single `er match` invocation calls `register_term_frequency_lookup` exactly once per `tf: true` column of `configs/test.yaml` (`given_name`, `family_name`, `email`) and calls `compute_tf_table` zero times, asserted with a counting spy on the linker's `table_management` namespace
- [ ] AC5: Every `match_scores` row written by both runs carries a non-NULL `tf_snapshot_id` equal to the `status='active'` `model_registry` row's value, and `SELECT count(DISTINCT tf_snapshot_id) FROM lake.main.match_scores` returns 1
- [ ] AC6: Zero relations matching `__splink__%` exist in `lake` after both runs

## Tests

- tests/integration/test_tf_policy.py::test_score_is_corpus_size_independent
- tests/integration/test_tf_policy.py::test_missing_tf_lookup_exits_3
- tests/integration/test_tf_policy.py::test_tf_registered_per_column_and_compute_tf_table_never_called

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_tf_policy.py -q
uv run mypy --strict src/er/matching/tf.py
uv run ruff check src/er tests && uv run ruff format --check src/er tests
```

## Definition of Done

- `assert_tf_lookup_complete` is called from the `er match` preflight in both modes and is unreachable-by-accident (no code path scores without it)
- The precondition message names every missing `column_name`, not just the first
- No probability constant is committed anywhere in the test — both arms are compared against each other, never against a literal
- `fixtures/static/model_test_v1.json` and `fixtures/static/base_10/base` are unmodified by this ticket
- `bash scripts/gates.sh` green; INTERFACES entry lists `assert_tf_lookup_complete`
