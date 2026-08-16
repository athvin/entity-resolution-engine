---
id: ER-056
title: "Committed fixture model + frozen tf_lookup + meta sidecar + regen_fixture_model.py + T-TRAIN-1 (slow-marked) + \"scenarios never train\" guard"
milestone: M3
status: in_progress
kind: code
size: L
gates: full
depends_on: ["ER-050", "ER-051", "ER-055"]
spec_refs: ["s8-3", "s4-3-2", "s4-3-3", "s12", "s10-1", "s10-2", "s6", "s2-1", "s8-2"]
gap_refs: ["M8", "M9", "M24", "D4"]
provides: ["fixtures/static/model_test_v1.json", "fixtures/static/model_test_v1.tf.csv", "fixtures/static/model_test_v1.meta.json", "scripts/regen_fixture_model.py", "tests/helpers/model.py::load_fixture_model", "tests/helpers/model.py::FIXTURE_MODEL_VERSION", "tests/helpers/model.py::FIXTURE_TF_SNAPSHOT_ID", "tests/unit/test_no_training_in_scenarios.py::TRAINING_ALLOWLIST"]
consumes: ["src/er/matching/train.py::train_model", "src/er/matching/train.py::run_train_stage", "src/er/lake/model_registry.py::register_model", "src/er/matching/tf.py::materialize_tf_lookup", "src/er/matching/tf.py::tf_tables_path", "fixtures/generator/emit.py::emit_corpus", "fixtures/generator/cli.py::main", "fixtures/generator/personas.py::generate_personas", "src/er/config/loader.py::load_config"]
owns: ["fixtures/static/model_test_v1.json", "fixtures/static/model_test_v1.tf.csv", "fixtures/static/model_test_v1.meta.json", "scripts/regen_fixture_model.py", "tests/helpers/model.py", "tests/integration/test_train_reproducibility.py", "tests/unit/test_no_training_in_scenarios.py", "tests/unit/fixtures/test_fixture_model.py"]
protected_paths: ["configs/test.yaml", "fixtures/static/base_10/", "fixtures/generator/"]
extra_paths: ["pyproject.toml", ".github/workflows/ci.yaml"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_train_reproducibility.py -m slow -q && uv run pytest tests/unit/test_no_training_in_scenarios.py -q"
branch: "ticket/ER-056-committed-fixture-model-frozen-tf-lookup"
commit: ""
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-16T16:14:44Z"
session: a63a301e-656f-4128-b5fd-844f1a369b46
---
## Description

Commits the frozen Splink model every M3 scenario test loads, together with its frozen `tf_lookup` rows and a meta sidecar pinning the six inputs T-TRAIN-1 (S8.3) requires to be identical for byte equality. S4.3.2 item 6 and S12 state the reason: scenario tests never train, because EM over `base_10`'s 23 records is degenerate — the u estimate draws from at most 253 pairs and m from at most 18. The model is trained once against the generator's 10k corpus (S10.2) under `configs/test.yaml`'s `generator.seed`, committed, and loaded thereafter. This ticket also ships the regeneration script, the slow-marked T-TRAIN-1 test, and the guard that keeps any scenario test from training.

## Scope

### In scope

- `scripts/regen_fixture_model.py [--check]`: generate the 10k corpus (personas 4,000 / records 10,000 per the S10.2 `10k` row) at `generator.seed`, ingest+standardize it into a disposable namespace, train, and write the three committed artifacts; `--check` regenerates and diffs instead of writing
- `fixtures/static/model_test_v1.json` — the settings JSON; `model_test_v1.tf.csv` — the frozen `tf_lookup` rows with header `model_version,tf_snapshot_id,column_name,value,tf_value`; `model_test_v1.meta.json` — the sidecar pinning corpus digest, `generator.seed`, scale, `model_version`, the verbatim `training:` block, `splink.__version__`, `duckdb.__version__`, `config_hash` and the SHA-256 of the model JSON
- `tests/helpers/model.py::load_fixture_model(conn)` — inserts the frozen `tf_lookup` rows and one `model_registry` row with `status='active'`, `params_path` pointing at the committed file, and returns `(model_version, tf_snapshot_id, settings)`; this is how every M3 scenario test acquires a model
- T-TRAIN-1 as `tests/integration/test_train_reproducibility.py`, marked `slow`, asserting each of the six pinned inputs before comparing bytes so a failure names which one diverged
- The "scenarios never train" guard: a source scan over `tests/integration/` with an explicit allowlist

### Out of scope

- Retraining, model lifecycle guards, T-MODEL-1 (ER-085)
- Changing `configs/test.yaml` (protected) — the model must match the committed config, not the other way round
- Re-authoring `base_10` or the generator (both protected)
- Committing benchmark corpora or baselines (M5)
- Any scenario expectation file

## Design decisions applied

Closes M8, M9, M24 and D4. Hard constraints: (1) the corpus is the generator's **10k** shape and the seed is `configs/test.yaml`'s `generator.seed` — S8.3 pins the corpus in the T-TRAIN-1 row precisely because byte equality is a claim about fully pinned inputs, and `benchmarks/scales.yaml` does not exist until M5, so the `(personas=4000, records=10000)` pair is written literally into the sidecar and the script; (2) the `model_version` the committed artifact carries is pinned by the sidecar, not left to the `max+1` allocation of S4.3.2; (3) the frozen `tf_lookup` rows are part of the artifact set — a scenario test that loads the model without them would have Splink compute TF and break INV-SCORE; (4) T-TRAIN-1 must be `slow`-marked and excluded from the PR path (M4 exit: the full PR path runs `-m "not slow"`). Import consumed symbols from INTERFACES.md; where a path differs, INTERFACES.md wins.

## Acceptance criteria

- [ ] AC1: `python scripts/regen_fixture_model.py --check` exits 0 against the committed artifacts and exits non-zero after a single byte of `fixtures/static/model_test_v1.json` is altered, naming the model artifact as the diverging output
- [ ] AC2: T-TRAIN-1 asserts all six pinned inputs before comparing bytes — corpus digest, `generator.seed`, scale, `model_version`, `training:` block, `splink.__version__` — and mutating any one of them (e.g. bumping the seed in a copy of the config) fails with a message naming that input rather than "bytes differ"
- [ ] AC3: `model_test_v1.meta.json`'s `sha256` field equals the SHA-256 of the committed `model_test_v1.json`, and its `training` object equals `configs/test.yaml`'s `training:` block in both directions
- [ ] AC4: `load_fixture_model(conn)` inserts exactly the rows of `model_test_v1.tf.csv` into `lake.main.tf_lookup` and exactly one `model_registry` row with `status='active'`; a spy asserts no Splink training method is invoked during the load
- [ ] AC5: `select distinct column_name from lake.main.tf_lookup` after `load_fixture_model` equals the `tf: true` column set of `configs/test.yaml` (`given_name`, `family_name`, `email`)
- [ ] AC6: The committed settings JSON declares `unique_id_column_name == 'record_key'`, and each of the six comparisons has a null level first and an else level last
- [ ] AC7: `uv run pytest --collect-only -q -m "not slow" tests/integration/test_train_reproducibility.py` collects zero tests, and the CI integration job's pytest invocation carries `-m "not slow"`
- [ ] AC8: The guard test fails when a file under `tests/integration/` outside the allowlist (`test_train.py`, `test_train_reproducibility.py`, `test_model_lifecycle.py`) references `er train`, `train_model` or `estimate_parameters_using_expectation_maximisation`; the negative arm is asserted with a synthetic file

## Tests

- tests/integration/test_train_reproducibility.py::test_fixture_model_regenerates_byte_for_byte
- tests/unit/test_no_training_in_scenarios.py::test_scenario_tests_never_train
- tests/unit/test_no_training_in_scenarios.py::test_guard_detects_a_planted_training_call
- tests/unit/fixtures/test_fixture_model.py::test_meta_sidecar_matches_committed_artifacts
- tests/unit/fixtures/test_fixture_model.py::test_training_block_in_meta_equals_config
- tests/unit/fixtures/test_fixture_model.py::test_tf_csv_covers_exactly_the_tf_true_columns
- tests/unit/fixtures/test_fixture_model.py::test_settings_declare_record_key_and_bracketed_levels

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_train_reproducibility.py -m slow -q && uv run pytest tests/unit/test_no_training_in_scenarios.py -q
uv run pytest tests/unit/fixtures/test_fixture_model.py -q
uv run python scripts/regen_fixture_model.py --check
```

## Definition of Done

- Acceptance criteria met and the verify command passes
- All three artifacts committed and byte-stable; `regen_fixture_model.py --check` is the only regeneration entry point
- `slow` marker registered in `pyproject.toml` and excluded from the CI integration invocation
- `tests/helpers/model.py` is the single way a scenario test acquires a model; no scenario test constructs a `model_registry` row itself
- `configs/test.yaml`, `fixtures/static/base_10/` and `fixtures/generator/` unmodified by this ticket's diff
- `provides` entries recorded in INTERFACES.md
- Committed on a branch off main
