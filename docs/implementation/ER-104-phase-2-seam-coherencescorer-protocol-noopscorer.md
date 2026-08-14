---
id: ER-104
title: "Phase-2 seam: CoherenceScorer Protocol, NoopScorer, name registry, reconcile hook (once per run), subject_type='entity' review rows"
milestone: M6
status: todo
kind: code
size: S
gates: full
depends_on: ["ER-063", "ER-074", "ER-092"]
spec_refs: ["s11", "s4-6", "s4-3-5", "s5", "s5-0", "s6", "s4-0", "s12"]
gap_refs: ["M20", "M2"]
provides: ["src/er/embeddings/__init__.py", "src/er/embeddings/coherence.py::ClusterCoherence", "src/er/embeddings/coherence.py::CoherenceScorer", "src/er/embeddings/coherence.py::NoopScorer", "src/er/embeddings/coherence.py::register_scorer", "src/er/embeddings/coherence.py::get_scorer", "src/er/embeddings/coherence.py::SCORER_REGISTRY", "src/er/golden/assemble.py::score_touched_entities", "tests/helpers/scorers.py::FakeScorer", "review_queue-row:(subject_type='entity', reason='coherence')"]
consumes: ["src/er/golden/assemble.py", "src/er/review/queue.py", "src/er/config/schema.py", "src/er/errors.py", "src/er/lake/ddl.py", "relation:er_touched_entities", "relation:review_queue", "tests/conftest.py", "tests/helpers/compare.py"]
owns: ["src/er/embeddings/coherence.py", "tests/helpers/scorers.py", "tests/unit/test_coherence_seam.py", "tests/integration/test_coherence_hook.py"]
protected_paths: ["tests/integration/test_touched_assembly.py", "tests/unit/review/test_queue_upsert.py"]
extra_paths: ["src/er/golden/assemble.py", "src/er/embeddings/__init__.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_coherence_hook.py -q && uv run pytest tests/unit/test_coherence_seam.py -q && uv run mypy --strict src/er/embeddings"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Ship the phase-2 seam of S11: the `ClusterCoherence` dataclass, the `CoherenceScorer` Protocol, the v1 `NoopScorer`, a name registry selected by `coherence.scorer` in S6, and the single call site inside `er assemble`. S11 pins the hook to assemble — immediately after `assemble.py` writes `er_touched_entities` and before the marts run, over the `disposition='rebuild'` ids, exactly once per run — and findings land as `review_queue` rows with `subject_type='entity'` under the S4.3.5 upsert rule. Closes M20's 'S11 routes entity-scoped findings into a pair-keyed table' and M2's requirement that the seam be exercised on every run without changing any output. No embedding implementation ships: S1 keeps that out of v1 scope.

## Scope

### In scope

- `src/er/embeddings/coherence.py`: `ClusterCoherence` (frozen dataclass), `CoherenceScorer` Protocol, `NoopScorer`, `SCORER_REGISTRY` with `register_scorer(name, factory)` / `get_scorer(cfg)`.
- The `er assemble` hook: construct the scorer once per run, call `score_clusters` once with the run's `disposition='rebuild'` entity ids in ascending `entity_id` order, after `er_touched_entities` is written and before the marts are invoked.
- Writing findings above the scorer's threshold as `review_queue` rows via the existing S4.3.5 upsert: `subject_type='entity'`, `entity_id` set, `rec_a_key`/`rec_b_key` NULL, `reason='coherence'`, `status='open'`, `waterfall` carrying dispersion and outlier record keys, `first_seen_run_id`/`last_seen_run_id`.
- `tests/helpers/scorers.py::FakeScorer` — a registered test double recording construction count, call count and the id list it received.
- Unit tests for the registry/protocol/Noop behaviour and an integration test for the hook, the row shape, the re-run refresh and the resolved-subject skip.

### Out of scope

- Any embedding, vector store, sentence-transformers dependency or DuckDB VSS usage (S1 out of scope; S11 ships the interface only).
- Letting coherence findings influence clustering, membership, golden output or the run's exit code.
- A second `review_queue` upsert implementation — reuse `src/er/review/queue.py` (its unit test is protected here).
- Calling the scorer from `reconcile`, per entity, or inside the label-propagation loop.
- Adding a coherence counter to `run_stages`: S4.6's counter set for `assemble` is normative and closed.
- New CLI verbs, config blocks or `review_queue` columns.

## Design decisions applied

Implements M20 (entity-scoped review subjects) and the S11 seam; M6's exit criterion is this ticket. The board row says 'reconcile hook' — **S11 overrides it**: the scorer is called from `er assemble`, and S11 states why (at reconcile time `er_touched_entities` holds no row for this `run_id`, so a scorer called there always receives an empty list). Other constraints: (1) exactly one construction and one `score_clusters` call per run, over `disposition='rebuild'` only — `retire` entities are being reaped and must not be scored; (2) pass the ids in ascending `entity_id` order via an explicit `ORDER BY`, because the review rows mint ULIDs and the mint order must be reproducible for idempotence and expected-file comparison, and S11 requires results returned in input order; (3) the upsert must refresh `last_seen_run_id` and skip subjects already `resolved_match`/`resolved_no_match`/`dismissed` (S4.3.5), and the S5.0 open-row logical key is `(subject_type, rec_a_key, rec_b_key, entity_id, reason)` so an entity may hold a `coherence` row independently of any pair row; (4) `NoopScorer` returns `dispersion=0.0` with no outliers and therefore writes zero rows — the code path runs on every run without changing any output; (5) an unregistered `coherence.scorer` value is a config error: `get_scorer` raises the taxonomy's config error (exit `2`, S4.0) naming the unknown value and the registered names; (6) `mypy --strict` must pass over the package with `CoherenceScorer` as a real `typing.Protocol` — no `Any`, no `cast`.

## Acceptance criteria

- [ ] AC1: With `coherence.scorer: noop`, `get_scorer(cfg)` returns a `NoopScorer`, and `NoopScorer().score_clusters(['E2','E1'])` returns two `ClusterCoherence` in that same input order with `dispersion == 0.0` and `outlier_record_keys == []`.
- [ ] AC2: `get_scorer(cfg)` with `coherence.scorer: does_not_exist` raises the config error class whose exit code is `2`, and the message contains both the unknown value and the registered names.
- [ ] AC3: On `merge_scenario` with `FakeScorer` registered and returning `dispersion` above its threshold for one entity, one `er run-all` produces exactly one `review_queue` row with `subject_type='entity'`, `reason='coherence'`, `status='open'`, NULL `rec_a_key` and `rec_b_key`, the finding's `entity_id`, and a `waterfall` containing its dispersion and outlier record keys, with `first_seen_run_id == last_seen_run_id == run_id`.
- [ ] AC4: `FakeScorer` records exactly one construction and exactly one `score_clusters` call for that run, and the id list it received equals `SELECT entity_id FROM lake.main.er_touched_entities WHERE run_id = :run AND disposition = 'rebuild' ORDER BY entity_id` — no `retire` entity appears.
- [ ] AC5: A second identical run refreshes `last_seen_run_id` on that row and inserts no second row (count remains 1); after `er review resolve … --as dismiss`, a third run inserts nothing and leaves the row `dismissed`.
- [ ] AC6: Under the default `noop` config a full `er run-all` writes zero `review_queue` rows with `reason='coherence'`, and the `assemble` stage's `run_stages.counters` key set equals exactly the six S4.6 names.
- [ ] AC7: `uv run mypy --strict src/er/embeddings` exits 0, and a function annotated to take `CoherenceScorer` accepts `NoopScorer` and `FakeScorer` without a runtime registration of either as a subclass.
- [ ] AC8: `tests/integration/test_touched_assembly.py` (T-INC-2) and the reconcile/membership assertions still pass unmodified with the hook active — coherence changes no membership, golden or event row.

## Tests

- tests/unit/test_coherence_seam.py::test_noop_scorer_returns_zero_dispersion_in_input_order
- tests/unit/test_coherence_seam.py::test_registry_returns_noop_by_default
- tests/unit/test_coherence_seam.py::test_unknown_scorer_name_is_config_error_exit_2
- tests/unit/test_coherence_seam.py::test_noop_satisfies_protocol_structurally
- tests/integration/test_coherence_hook.py::test_scorer_called_once_with_rebuild_touched_ids
- tests/integration/test_coherence_hook.py::test_finding_lands_as_entity_review_row
- tests/integration/test_coherence_hook.py::test_rerun_refreshes_and_skips_resolved_subject
- tests/integration/test_coherence_hook.py::test_noop_writes_no_rows_and_no_extra_counters

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_coherence_hook.py -q && uv run pytest tests/unit/test_coherence_seam.py -q && uv run mypy --strict src/er/embeddings
bash scripts/ci/itest.sh tests/integration/test_touched_assembly.py -q
bash scripts/gates.sh --ticket ER-104
```

## Definition of Done

- `src/er/embeddings/coherence.py` ships `ClusterCoherence`, the `CoherenceScorer` Protocol, `NoopScorer` and the name registry; `mypy --strict` clean.
- The hook lives in `assemble.py`, runs once per run after `er_touched_entities` is written and before the marts, over `disposition='rebuild'` ids ordered by `entity_id`.
- Findings written through the existing S4.3.5 upsert with `subject_type='entity'`, `reason='coherence'`; no second upsert implementation.
- Unknown `coherence.scorer` fails as a config error with exit code `2`.
- `noop` default writes zero rows and adds no `run_stages` counter; T-INC-2 and the reconcile suite unchanged and green.
- No embedding dependency added to `pyproject.toml`/`uv.lock`.
- `scripts/gates.sh --ticket ER-104` green (full scope) with a receipt.
