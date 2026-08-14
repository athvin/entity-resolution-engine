---
id: ER-065
title: "Incremental match: two Splink passes unioned (find_matches_to_new_records + batch-only dedupe_only)"
milestone: M3
status: todo
kind: code
size: L
gates: full
depends_on: ["ER-057", "ER-058", "ER-059", "ER-064"]
spec_refs: ["s4-3", "s4-3-3", "s4-3-4", "s4-3-5", "s4-0", "s4-0b", "s5", "s5-0", "s5-2", "s8-2", "s12-1"]
gap_refs: ["B3", "D2"]
provides: ["src/er/matching/incremental.py::score_incremental", "src/er/matching/incremental.py::pass1_new_vs_corpus", "src/er/matching/incremental.py::pass2_new_vs_new", "src/er/matching/incremental.py::IncrementalScoreResult", "src/er/matching/incremental.py::unscored_record_keys", "cli:er match --mode incremental", "tests/integration/test_incremental_match.py::test_two_passes_are_unioned"]
consumes: ["src/er/matching/model.py::blocking_rules_from_config", "src/er/matching/model.py::build_settings", "src/er/matching/full.py::merge_match_scores", "src/er/matching/full.py::score_full", "src/er/matching/tf.py::register_tf", "src/er/matching/tf.py::assert_tf_lookup_complete", "src/er/matching/edges.py::current_edges", "src/er/lake/ducklake.py::splink_api", "src/er/lake/ducklake.py::connect", "src/er/entities/ids.py::canonicalize_pair", "src/er/review/queue.py::upsert_gray_band_pairs", "src/er/obs/run_context.py::RunContext", "src/er/errors.py::PreconditionFailure", "fixture:incremental_batch", "fixtures/static/model_test_v1.json", "tests/helpers/pairs.py::blocked_pairs", "tests/helpers/scenarios.py::load_scenario", "tests/conftest.py::lake_conn", "relation:match_scores", "relation:int_std_records"]
owns: ["src/er/matching/incremental.py", "tests/integration/test_incremental_match.py"]
protected_paths: ["fixtures/static/incremental_batch", "fixtures/static/model_test_v1.json"]
extra_paths: ["src/er/cli.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_incremental_match.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

D2 fixes incremental scoring as two Splink passes over the same frozen model and the same registered TF tables: `find_matches_to_new_records` for new-vs-corpus and a batch-only `link_type='dedupe_only'` Linker for new-vs-new, unioned, self-pairs dropped, canonicalised, DISTINCT, and persisted with one `MERGE INTO lake.main.match_scores` (S4.3.4). Pass 2 exists because `find_matches_to_new_records` never pairs new records with each other, which is exactly the `incremental_batch` case where two new records form a new entity. This ticket implements `src/er/matching/incremental.py` and the `er match --mode incremental` behaviour, and proves both passes contribute.

## Scope

### In scope

- Pass 1: `find_matches_to_new_records(records_or_tablename='batch_std', blocking_rules=blocking_rules_from_config(cfg)[1], match_weight_threshold=log2(review_low/(1-review_low)))`
- Pass 2: a second Linker over the batch alone with the frozen settings switched to `link_type='dedupe_only'`, same registered TF tables, `predict(threshold_match_probability=review_low)`
- Union, self-pair drop, canonicalisation to `rec_a_key < rec_b_key`, `DISTINCT`, and one `MERGE INTO` on `(model_version, tf_snapshot_id, rec_a_key, rec_b_key)` carrying both endpoint content hashes, `evidence`, `is_active=true`, `run_id`, `scored_at`
- `unscored_record_keys` driving the S4.0 exit `10` (incremental mode with nothing to score) and the `run_stages` counter payload of S4.3.5
- Gray-band pairs handed to `upsert_gray_band_pairs`; nothing in the band is clustered or persisted as an above-threshold edge

### Out of scope

- Scoring parity against the full path and orientation invariance (ER-066)
- Affected node/edge computation, clustering and reconcile (ER-069, ER-070, ER-071, ER-074)
- Edge invalidation on supersession or deletion (ER-082, ER-083)
- Minting a `tf_snapshot_id` or retraining — the run uses the `status='active'` registry row
- Changing `full.py`'s corpus-wide path beyond reusing its writer

## Design decisions applied

Implements D2 and gap-report B3. Constraints: (1) `int_blocking_keys` is NOT an input to scoring — Splink regenerates its own candidates from the same rules (S4.3.4); a pair table handed to Splink is a rewrite of the ticket, not an implementation of it; (2) pass 1 takes a match WEIGHT and pass 2 a match PROBABILITY — pass `log2(p/(1-p))` to the first and `review_low` to the second, and never rely on Splink's `-4` default (S4.3); (3) both passes run against the same frozen model JSON and the same registered `tf_lookup`, with `assert_tf_lookup_complete` in the preflight; (4) `match_scores` is cumulative and MUST NOT be truncated per run — the write is a MERGE on the four-column key, at most one row per key regardless of `is_active` (S4.3.4, S5.0); (5) all Splink work happens on the `:memory:` primary database with `output_schema='splink_scratch'`, and only the final scored pairs reach the lake in a single write statement (S4.0b).

## Acceptance criteria

- [ ] AC1: After loading `incremental_batch/base/` and running the base full match, `er match --mode incremental` over `batch/` exits `0` and every persisted new row has at least one endpoint in `batch/`
- [ ] AC2: The pair formed by the two `new_pair` batch records (both endpoints in `batch/`) is present in `match_scores` at or above `auto_merge`; deleting the pass-2 Linker from the implementation makes this assertion fail — asserted by a second test that monkeypatches `pass2_new_vs_new` to return an empty frame and expects the pair to be absent
- [ ] AC3: Every row written carries `rec_a_key < rec_b_key`, no self-pairs, exactly one row per `(model_version, tf_snapshot_id, rec_a_key, rec_b_key)`, both endpoint `content_hash` values equal to the current `int_std_records` values, `is_active=true`, and an `evidence` object containing the `gamma_*` vector
- [ ] AC4: No persisted row has `match_probability < review_low`, and every pair in `[review_low, auto_merge)` has exactly one open `review_queue` row with `reason='gray_band'` and is absent from the above-`auto_merge` edge set
- [ ] AC5: The rows written by the preceding base full run are still present and `is_active=true` after the incremental run (row count strictly increases; nothing is truncated)
- [ ] AC6: Re-running `er match --mode incremental` over the same batch rewrites the same keys with exactly equal `match_probability`, adds zero rows, and a third run with no unscored records exits `10`
- [ ] AC7: `run_stages` for the match stage carries `candidate_pairs`, `pairs_above_auto_merge` and `review_queue_added` as typed columns and a `counters` JSON containing `mode`, `model_version`, `tf_snapshot_id`, `pairs_scored`, `pairs_in_gray_band`, `review_queue_refreshed` and `duration_ms`
- [ ] AC8: Zero relations matching `__splink__%` exist in `lake` after the run

## Tests

- tests/integration/test_incremental_match.py::test_two_passes_are_unioned
- tests/integration/test_incremental_match.py::test_pass2_removal_loses_the_new_pair
- tests/integration/test_incremental_match.py::test_persisted_rows_are_canonical_and_keyed
- tests/integration/test_incremental_match.py::test_gray_band_is_queued_not_clustered
- tests/integration/test_incremental_match.py::test_match_scores_is_cumulative_and_idempotent
- tests/integration/test_incremental_match.py::test_counters_and_no_splink_relations

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_incremental_match.py -q
uv run mypy --strict src/er/matching/incremental.py
uv run ruff check src/er tests && uv run ruff format --check src/er tests
```

## Definition of Done

- `int_blocking_keys` is not read by `incremental.py` (grep-clean)
- The weight/probability conversion exists in one helper and is used by pass 1 only
- The persist step is a single `MERGE INTO` statement — no per-pair writes
- `fixtures/static/incremental_batch/` and the committed model are unmodified by this ticket
- `bash scripts/gates.sh` green; INTERFACES entry lists `score_incremental` and both pass functions
