---
id: ER-054
title: "er train part A: training: block consumption + ordered Splink 4 call sequence (spy-verified), seeded estimate_u, thread-pinned"
milestone: M3
status: done
kind: code
size: M
gates: fast
depends_on: ["ER-011", "ER-051", "ER-053"]
spec_refs: ["s4-3-2", "s6", "s6-1", "s4-0", "s4-0b", "s4-3-1", "s2-1"]
gap_refs: ["M8", "M26"]
provides: ["src/er/matching/train.py::train_model", "src/er/matching/train.py::TrainResult", "src/er/matching/train.py::TRAIN_CALL_SEQUENCE", "src/er/matching/train.py::build_training_linker"]
consumes: ["src/er/config/schema.py::ErConfig", "src/er/config/schema.py::TrainingConfig", "src/er/config/loader.py::load_config", "src/er/matching/model.py::build_settings", "src/er/matching/model.py::blocking_rules_from_config", "src/er/matching/splink_env.py::splink_api", "src/er/matching/tf.py::materialize_tf_lookup", "src/er/matching/tf.py::new_tf_snapshot_id", "fixtures/generator/emit.py::emit_corpus"]
owns: ["src/er/matching/train.py", "tests/unit/matching/test_train_sequence.py", "tests/unit/config/test_training_block.py"]
protected_paths: []
extra_paths: ["src/er/config/schema.py"]
attempts: 1
verify: "uv run pytest tests/unit/matching/test_train_sequence.py tests/unit/config/test_training_block.py -q"
branch: "ticket/ER-054-er-train-part-training-block-consumption"
commit: "8bea5ab"
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-16T13:45:45Z"
session: 178d0651-7261-407d-af50-43dc182f35a2
---
## Description

Builds the pure, testable core of `er train`: the ordered Splink 4 call sequence of S4.3.2 driven entirely from the `training:` block of S6, verified against a spy linker so the order, the namespaced call names and every keyword argument are pinned. M8 in the gap report is precisely that `estimate_probability_two_random_records_match` and `estimate_parameters_using_expectation_maximisation` have required arguments the old config could not supply and that `estimate_u_using_random_sampling` was unseeded, making two trainings on one corpus produce different models. This ticket makes the sequence executable, seeded and reproducible; ER-055 gives its output a durable home.

## Scope

### In scope

- `train_model(conn, cfg, corpus_relation, *, model_version, tf_snapshot_id)` executing exactly the S4.3.2 sequence: `estimate_probability_two_random_records_match(deterministic_matching_rules=..., recall=...)` → `estimate_u_using_random_sampling(max_pairs=..., seed=...)` → one `estimate_parameters_using_expectation_maximisation(blocking_rule=..., fix_u_probabilities=...)` per `em_blocking_rules` entry, in config order
- `TRAIN_CALL_SEQUENCE` — the declared expected call list the spy test compares against
- `TrainResult` carrying the fitted settings JSON (dict), the verbatim `training:` block, and the fit metrics destined for `model_registry.metrics`
- `build_training_linker` obtaining its `DuckDBAPI` from `splink_api()` so the primary database stays `:memory:`, `output_schema='splink_scratch'`, and `threads` is pinned from `ER_DUCKDB_THREADS` (S4.0b)
- TF materialization invoked from the training path (the only permitted caller per D4)
- Coverage for V9/V10 of S6.1 in `tests/unit/config/test_training_block.py`, adding the validators to `src/er/config/schema.py` only if they are absent

### Out of scope

- `model_version` allocation, S3 upload, `model_registry` writes, active/superseded pointer, `corpus_snapshot` capture, `run_stages` (all ER-055)
- `er train` CLI wiring and exit codes (ER-055)
- Regenerating or committing the fixture model (ER-056)
- Redefining the `training:` Pydantic block — ER-011 owns the schema; extend it only if V9/V10 are missing
- Any lake write beyond the `tf_lookup` materialization ER-053 owns

## Design decisions applied

Closes M8 (training arguments come from config, `u_seed` required) and M26 (`training:` is a config block, not a scattered set of literals). Constraints that are easy to miss: `estimate_u_using_random_sampling` MUST receive `seed=cfg.training.u_seed` — the Splink default is `None` and an unseeded u estimate makes T-TRAIN-1 unachievable; at least two EM sessions are required because m is not estimated for blocked columns (V9); the whole `training:` block is persisted verbatim into `model_registry.metrics` (S4.3.2), so `TrainResult` must carry it unmodified rather than re-derived. Splink 4 call names are namespaced (`linker.training.*`) — a call on the bare `linker` is the Splink 3 API and fails the spy assertion. Import consumed symbols from INTERFACES.md; where a path differs, INTERFACES.md wins.

## Acceptance criteria

- [ ] AC1: Against a spy linker, the recorded `(method_path, kwargs)` list from `train_model` equals `TRAIN_CALL_SEQUENCE` rendered from `configs/test.yaml` — same order, same namespaced names (`training.estimate_probability_two_random_records_match`, `training.estimate_u_using_random_sampling`, `training.estimate_parameters_using_expectation_maximisation`), same keyword values
- [ ] AC2: `estimate_u_using_random_sampling` is always called with `seed` equal to `cfg.training.u_seed` and `max_pairs` equal to `cfg.training.u_max_pairs`; a spy recording `seed=None` fails the test
- [ ] AC3: One EM call is issued per entry of `training.em_blocking_rules`, in config order, each with `fix_u_probabilities=cfg.training.em.fix_u_probabilities`; reordering the config list reorders the recorded calls
- [ ] AC4: A config missing `training.u_seed` fails validation with error key `training.u_seed.required` and exit code 2; a config with one `em_blocking_rules` entry fails with `training.em_blocking_rules.min_items`; `recall` outside `(0, 1]` fails
- [ ] AC5: `TrainResult.metrics['training']` equals the loaded config's `training` block as a dict, in both directions, with no key added or dropped
- [ ] AC6: `build_training_linker` obtains its API from `splink_api()`: a test asserting the linker's `db_api` was constructed with `output_schema='splink_scratch'` and that the connection's `threads` setting equals `ER_DUCKDB_THREADS` passes, and constructing `DuckDBAPI` directly in `train.py` fails a source-scan assertion
- [ ] AC7: Two `train_model` invocations over the same in-memory corpus with the same config produce identical settings JSON (`json.dumps(..., sort_keys=True)` equality)

## Tests

- tests/unit/matching/test_train_sequence.py::test_call_sequence_is_exact_and_ordered
- tests/unit/matching/test_train_sequence.py::test_estimate_u_is_seeded_from_config
- tests/unit/matching/test_train_sequence.py::test_one_em_session_per_blocking_rule_in_order
- tests/unit/matching/test_train_sequence.py::test_training_block_persisted_verbatim
- tests/unit/matching/test_train_sequence.py::test_linker_uses_splink_scratch_and_pinned_threads
- tests/unit/matching/test_train_sequence.py::test_repeat_training_is_bit_identical
- tests/unit/config/test_training_block.py::test_missing_u_seed_rejected
- tests/unit/config/test_training_block.py::test_em_blocking_rules_min_items
- tests/unit/config/test_training_block.py::test_recall_bounds_rejected

## Verification

```bash
uv run pytest tests/unit/matching/test_train_sequence.py tests/unit/config/test_training_block.py -q
uv run mypy --strict src/er/matching/train.py
```

## Definition of Done

- Acceptance criteria met and the verify command passes
- No Splink 3-style bare `linker.<method>` inference/training call anywhere in `train.py`
- `DuckDBAPI` is constructed only inside `splink_api()`; `train.py` never constructs one
- `training:` block flows into `TrainResult.metrics` unmodified — no re-serialisation that reorders or coerces values
- `mypy --strict src/er/matching/train.py` clean
- `provides` entries recorded in INTERFACES.md
- Committed on a branch off main
