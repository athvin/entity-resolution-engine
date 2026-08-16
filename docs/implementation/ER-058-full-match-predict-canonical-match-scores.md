---
id: ER-058
title: "Full match: predict → canonical match_scores single INSERT, evidence payload schema, half-open gray band, prob_to_weight, counters"
milestone: M3
status: in_progress
kind: code
size: M
gates: full
depends_on: ["ER-023", "ER-043", "ER-047", "ER-053", "ER-056", "ER-063"]
spec_refs: ["s4-3", "s4-3-3", "s4-3-4", "s4-3-5", "s4-0b", "s5", "s5-0", "s5-2", "s4-0"]
gap_refs: ["M9", "M17", "MINOR-thresholds"]
provides: ["src/er/matching/full.py::score_full", "src/er/matching/thresholds.py::prob_to_weight", "src/er/matching/thresholds.py::weight_to_prob", "src/er/matching/thresholds.py::in_gray_band", "src/er/matching/thresholds.py::is_auto_merge", "src/er/matching/evidence.py::build_evidence", "src/er/matching/evidence.py::EVIDENCE_KEYS", "cli:er match --mode full"]
consumes: ["src/er/matching/tf.py::register_tf", "src/er/matching/tf.py::new_tf_snapshot_id", "src/er/matching/model.py::build_settings", "src/er/matching/model.py::blocking_rules_from_config", "src/er/matching/splink_env.py::splink_api", "src/er/matching/splink_env.py::assert_no_splink_relations_in_lake", "src/er/entities/ids.py::canonicalize_pair", "src/er/review/queue.py::upsert_review", "src/er/lake/model_registry.py::active_model", "src/er/lake/model_registry.py::load_model_settings", "src/er/obs/run_context.py::RunContext", "tests/helpers/model.py::load_fixture_model", "tests/helpers/pairs.py::canonical_pairs_from_blocking_keys"]
owns: ["src/er/matching/full.py", "src/er/matching/thresholds.py", "src/er/matching/evidence.py", "tests/integration/test_full_match.py", "tests/unit/matching/test_thresholds.py"]
protected_paths: []
extra_paths: ["src/er/cli.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_full_match.py -q"
branch: "ticket/ER-058-full-match-predict-canonical-match-scores"
commit: ""
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-16T21:52:06Z"
session: 9e0ef860-5643-4d7b-b395-c829fa4931f6
---
## Description

Implements `er match --mode full`: one corpus-wide `linker.inference.predict(threshold_match_probability=review_low)` over the frozen model with registered TF, canonicalised and persisted to `lake.main.match_scores` in a single write statement (S4.3.4, S4.0b), with the gray band routed to `review_queue` (S4.3.5). It closes M17 (no `__splink__` relation may reach the lake and only final scored pairs are written), M9 (every row carries `model_version`, `tf_snapshot_id` and both endpoint content hashes so INV-SCORE is checkable after the fact) and MINOR-thresholds (config thresholds are probabilities; where Splink takes a weight, pass `log2(p/(1-p))` and never rely on the -4 default; the gray band is half-open).

## Scope

### In scope

- `score_full(conn, cfg, run_ctx, *, model_version, tf_snapshot_id)` — register TF, predict at `threshold_match_probability=review_low`, drop self-pairs, canonicalise to `rec_a_key < rec_b_key`, DISTINCT, and persist in exactly one write statement (`MERGE INTO` on the S5.0 logical key `(rec_a_key, rec_b_key, model_version, tf_snapshot_id)`)
- Row payload: `match_probability`, `model_version`, `tf_snapshot_id`, `rec_a_content_hash`, `rec_b_content_hash`, `evidence JSON`, `is_active=true`, `run_id`, `scored_at`
- `src/er/matching/thresholds.py`: `prob_to_weight`, `weight_to_prob`, `in_gray_band` (half-open `review_low <= p < auto_merge`), `is_auto_merge` (`p >= auto_merge`) — the single definitions every other module imports
- `src/er/matching/evidence.py`: the `evidence`/`waterfall` payload — one `gamma_<col>` per configured comparison plus per-comparison Bayes factors and the match weight, retained rather than projected away (S4.3.5)
- Gray-band pairs upserted to `review_queue` with `subject_type='pair'`, `reason='gray_band'` and not clustered
- `er match --mode full` CLI wiring: exit 3 with no active model, exit 1 on scoring failure, stdout line per S4.0, `run_stages` counters per S4.3.5

### Out of scope

- Incremental two-pass scoring (ER-065) — but `thresholds.py` and `evidence.py` are written so it imports them
- `--new-tf-snapshot` TF rebuild semantics (ER-094); the flag may be accepted and rejected as unimplemented here only if it is not silently ignored
- Clustering, membership, entities (ER-070..ER-074)
- Edge invalidation on `content_hash` change (ER-082)
- Quality metrics over the scored set (ER-067)

## Design decisions applied

Closes M9, M17 and MINOR-thresholds. Constraints easy to miss: (1) the board title says "single INSERT" and S4.3.4 says `MERGE INTO` — they mean the same requirement, **one write statement** against `lake.main.match_scores` per stage; DuckLake's MERGE supports a single `when_matched` action, which is sufficient here; (2) `match_scores` is cumulative and is NEVER truncated per run, and invalidation is an in-place `UPDATE`, never a second row — at most one row per logical key regardless of `is_active`; (3) nothing below `review_low` is persisted; (4) the gray band is half-open, so `p == auto_merge` is a match and `p == review_low` is a review row; (5) Splink must receive `match_weight_threshold`/`threshold_match_probability` explicitly — relying on the -4 default is a defect; (6) the connection comes from `splink_api()` so no `__splink__` relation lands in the lake. Import consumed symbols from INTERFACES.md; where a path differs, INTERFACES.md wins.

## Acceptance criteria

- [ ] AC1: After `er match --mode full` on `base_10` with the committed model, every `match_scores` row satisfies `rec_a_key < rec_b_key`, `is_active = true`, non-null `model_version`, `tf_snapshot_id`, `rec_a_content_hash`, `rec_b_content_hash`, and `match_probability >= thresholds.review_low`; zero rows fall below `review_low`
- [ ] AC2: Re-running `er match --mode full` at the same `(model_version, tf_snapshot_id)` leaves the row count unchanged, every `match_probability` bit-identical, and every non-`VOLATILE_COLUMNS` value unchanged; `select count(*) - count(distinct (rec_a_key, rec_b_key, model_version, tf_snapshot_id))` is 0
- [ ] AC3: A statement spy on the lake connection records exactly one write statement targeting `lake.main.match_scores` for the stage
- [ ] AC4: Every row's `evidence` JSON contains one `gamma_<col>` key and one Bayes-factor key per configured comparison plus `match_weight`; the set of `gamma_*` suffixes equals the `comparisons` key set of the loaded config
- [ ] AC5: No `review_queue` row has `match_probability >= auto_merge` or `< review_low`; `in_gray_band(auto_merge)` is False and `in_gray_band(review_low)` is True (unit)
- [ ] AC6: `prob_to_weight(p)` equals `log2(p/(1-p))` within 1e-12 for `p ∈ {0.60, 0.95}`, `weight_to_prob(prob_to_weight(p)) == p` within 1e-12, and a spy asserts the threshold passed to Splink is derived from `review_low` rather than left at the -4 default
- [ ] AC7: The stage's `run_stages` row carries `candidate_pairs` and `pairs_above_auto_merge` as typed columns and a `counters` JSON containing `mode`, `model_version`, `tf_snapshot_id`, `pairs_scored`, `pairs_in_gray_band`, `review_queue_added` and `duration_ms`; `er match` with no active model exits 3
- [ ] AC8: `assert_no_splink_relations_in_lake(conn)` passes after the stage

## Tests

- tests/integration/test_full_match.py::test_rows_are_canonical_keyed_and_above_review_low
- tests/integration/test_full_match.py::test_rescoring_is_idempotent_on_the_logical_key
- tests/integration/test_full_match.py::test_single_write_statement_to_match_scores
- tests/integration/test_full_match.py::test_evidence_payload_covers_every_comparison
- tests/integration/test_full_match.py::test_gray_band_is_half_open_and_lands_in_review_queue
- tests/integration/test_full_match.py::test_counters_and_no_active_model_exit_3
- tests/integration/test_full_match.py::test_no_splink_relations_in_lake
- tests/unit/matching/test_thresholds.py::test_prob_to_weight_round_trip
- tests/unit/matching/test_thresholds.py::test_gray_band_boundaries_are_half_open

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_full_match.py -q
uv run pytest tests/unit/matching/test_thresholds.py -q
uv run mypy --strict src/er/matching/full.py src/er/matching/thresholds.py src/er/matching/evidence.py
```

## Definition of Done

- Acceptance criteria met and the verify command passes
- Exactly one write statement to `match_scores` per stage, asserted by a spy rather than by inspection
- `thresholds.py` holds the only definitions of the gray band and the probability/weight conversion; no second copy anywhere
- `match_scores` is never truncated and no code path deletes from it
- `er match --mode full` documented with S4.0 flags, exit codes and stdout line
- `mypy --strict` clean on the three new modules
- `provides` entries recorded in INTERFACES.md
- Committed on a branch off main
