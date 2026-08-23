---
id: ER-060
title: "**NEW** base_10 trap verification against the committed model: exactly one gray-band pair, ≥1 single-rule-covered true pair, tie row, recency never decides"
milestone: M3
status: in_progress
kind: code
size: S
gates: full
depends_on: ["ER-041", "ER-043", "ER-047", "ER-056", "ER-057", "ER-058"]
spec_refs: ["s8-2", "s8-3", "s4-3-5", "s4-6", "s6", "s8-2-1"]
gap_refs: ["M21", "M11", "B4"]
provides: ["tests/integration/test_base_10_traps.py::TRAP_IDS", "tests/helpers/traps.py::load_trap_index", "tests/helpers/traps.py::true_pairs_from_truth"]
consumes: ["fixtures/static/base_10/truth.csv", "fixtures/static/base_10/base/crm.csv", "tests/helpers/model.py::load_fixture_model", "tests/helpers/pairs.py::canonical_pairs_from_blocking_keys", "src/er/matching/full.py::score_full", "src/er/matching/thresholds.py::in_gray_band", "src/er/matching/thresholds.py::is_auto_merge", "src/er/config/loader.py::load_config", "tests/conftest.py::lake_ns"]
owns: ["tests/integration/test_base_10_traps.py", "tests/helpers/traps.py"]
protected_paths: ["fixtures/static/base_10/", "src/er/matching/", "configs/test.yaml"]
extra_paths: []
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_base_10_traps.py -q"
branch: "ticket/ER-060-new-base-10-trap-verification-against"
commit: ""
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-23T09:34:48Z"
session: c379438e-8c95-4be8-8f5f-3832d8ceafaa
---
## Description

`base_10`'s designed traps (S8.2) are properties of the fixture **under the committed model**, so they are circular to assert at fixture-authoring time and are silently relied on by six downstream tests — T-MATCH-1a/1b, T-REVIEW-1, T-GOLD-1 and the two incremental arms. This ticket makes them a post-match gate: after `er match --mode full` with `fixtures/static/model_test_v1.json`, each trap in the S8.2 table is asserted by name, along with the two normative authoring constraints (the gray-band pair MUST be cross-persona; a tolerated missed edge MUST lie inside a persona of three or more records) and the survivorship-tie and recency preconditions T-GOLD-1 will later depend on.

## Scope

### In scope

- Exactly one pair scores in the half-open gray band, and its endpoints belong to two different `persona_id`s per `truth.csv`
- At least one true persona pair is covered by exactly one `key_type` — the single-rule-covered pair, so a blocking-rule regression is visible rather than masked by a second rule
- Every true pair that scores below `auto_merge` (or is absent from `match_scores`) lies inside a persona of ≥3 records and its removal leaves that persona's `>= auto_merge` edges connected — the S8.2 authoring constraint
- The named traps under the committed model: robert/bob `>= auto_merge`; the typo-surname pair `>= auto_merge`; the shared-household pair has no edge at or above `auto_merge`; the two `test@test.com` records have `email IS NULL` in `int_std_records` and no `match_scores` row between them; the drifted phones normalise to one value and block together; the four empty-email records emit no `email_exact` blocking key
- The survivorship tie row exists structurally in `int_std_records` (same persona, same source, equal `priority_rank`, equal `COALESCE(updated_at_source, ingested_at)`, both non-null equal-length differing `given_name`)
- `recency` never decides on volatile data: no contest an attribute's `recency` rule could decide is decided by the `ingested_at` fallback
- One `TRAP-<name>` id per assertion and a completeness check that the asserted id set equals the eight rows of the S8.2 trap table

### Out of scope

- Computing precision/recall — T-MATCH-1a/1b own the metrics and `pairwise_metrics` (ER-067/ER-081)
- Asserting anything about `golden_records`/`golden_lineage`: those relations do not exist until M4, so the recency and tie traps are asserted over `int_std_records` and the config chains only
- Editing the fixture or the matching code to make a trap hold — both protected; a failure is a blocker against ER-041
- The `review_queue` row shape (T-REVIEW-1, ER-086) — this ticket asserts the score band, not the queue row

## Design decisions applied

Closes M21 (traps as named sub-assertions with absolute counts), M11 (the tie and recency preconditions that make T-GOLD-1 deterministic) and B4's fixture arm. Two S8.2 authoring constraints are normative and MUST be asserted, not assumed: the gray-band pair is cross-persona (a same-persona one would make entity count 11 and fail T-MATCH-1b with precision still 1.0), and any tolerated missed edge lies inside a persona of ≥3 records and is transitively recoverable. `recency`'s literal fragment is `COALESCE(updated_at_source, ingested_at) DESC` and `ingested_at` is in `VOLATILE_COLUMNS` (S5.0) — a contest decided by the fallback is non-deterministic across runs, which is why this ticket forbids it on `base_10`. Import consumed symbols from INTERFACES.md; where a path differs, INTERFACES.md wins.

## Acceptance criteria

- [ ] AC1: Exactly one `match_scores` row satisfies `in_gray_band(match_probability)` on `base_10`, and its two `record_key`s map to different `persona_id`s in `truth.csv`
- [ ] AC2: At least one of the 18 true pairs appears in the pair set of exactly one `key_type`, and the test prints which pair and which rule
- [ ] AC3: Every true pair whose score is below `auto_merge` or absent belongs to a persona with ≥3 records, and removing it leaves that persona's `>= auto_merge` edges connected; zero such pairs also satisfies the assertion
- [ ] AC4: The robert/bob pair and the typo-surname pair each have a `match_scores` row with `match_probability >= auto_merge`; the shared-household pair has none; the two `test@test.com` records have `email IS NULL` in `int_std_records` and no `match_scores` row between them
- [ ] AC5: The three drifted phone spellings of one persona all appear as `+14155550132` in `int_std_records` and share a `phone_exact` blocking key; the four empty-email records produce zero `email_exact` blocking rows
- [ ] AC6: Exactly one persona has two `int_std_records` rows from the same `source_system` with equal `priority_rank`, equal `COALESCE(updated_at_source, ingested_at)`, and two non-null, equal-length, differing `given_name` values
- [ ] AC7: For every attribute whose survivorship chain contains `recency`, no contest between two member rows is decided by `COALESCE` falling back to `ingested_at` — every recency-relevant row carries a non-null `updated_at_source`, or the contest is already decided earlier in the chain
- [ ] AC8: The set of `TRAP-<name>` ids asserted by the module equals the eight rows of the S8.2 designed-traps table; adding a trap to the spec without an assertion here fails the completeness test

## Tests

- tests/integration/test_base_10_traps.py::test_exactly_one_gray_band_pair_and_it_is_cross_persona
- tests/integration/test_base_10_traps.py::test_at_least_one_true_pair_is_single_rule_covered
- tests/integration/test_base_10_traps.py::test_missed_true_pairs_satisfy_the_authoring_constraint
- tests/integration/test_base_10_traps.py::test_named_score_traps_hold_under_committed_model
- tests/integration/test_base_10_traps.py::test_phone_drift_and_missing_email_blocking_traps
- tests/integration/test_base_10_traps.py::test_survivorship_tie_row_exists
- tests/integration/test_base_10_traps.py::test_recency_never_decides_on_ingested_at
- tests/integration/test_base_10_traps.py::test_trap_ids_cover_the_s8_2_table

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_base_10_traps.py -q
uv run pytest tests/unit/fixtures/test_base_10.py -q
```

## Definition of Done

- Acceptance criteria met and the verify command passes
- Every assertion carries a `TRAP-<name>` id and the completeness test enforces the S8.2 table
- Trap failures print the offending `record_key`s and probabilities, not just a boolean
- Protected paths unmodified; a failing trap is filed as a blocker against ER-041, not patched here
- No assertion reads `golden_records`, `golden_lineage` or `entity_membership`
- `provides` entries recorded in INTERFACES.md
- Committed on a branch off main
