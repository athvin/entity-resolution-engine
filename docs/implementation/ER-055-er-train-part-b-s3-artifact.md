---
id: ER-055
title: "er train part B: S3 artifact URI, model_version allocation, active/superseded pointer, corpus_snapshot, run_stages"
milestone: M3
status: in_progress
kind: code
size: M
gates: full
depends_on: ["ER-015", "ER-023", "ER-054"]
spec_refs: ["s4-3-2", "s5", "s5-0", "s5-2", "s4-0", "s4", "s6", "s6-1"]
gap_refs: ["M8"]
provides: ["src/er/lake/model_registry.py::allocate_model_version", "src/er/lake/model_registry.py::register_model", "src/er/lake/model_registry.py::active_model", "src/er/lake/model_registry.py::load_model_settings", "src/er/lake/model_registry.py::ModelRow", "src/er/matching/train.py::run_train_stage", "cli:er train"]
consumes: ["src/er/matching/train.py::train_model", "src/er/matching/train.py::TrainResult", "src/er/matching/tf.py::materialize_tf_lookup", "src/er/matching/tf.py::new_tf_snapshot_id", "src/er/matching/tf.py::tf_tables_path", "src/er/lake/objectstore.py::put_object", "src/er/lake/objectstore.py::get_object", "src/er/lake/ducklake.py::current_snapshot", "src/er/obs/run_context.py::RunContext", "src/er/config/hashing.py::config_hash", "src/er/cli.py::app"]
owns: ["src/er/lake/model_registry.py", "tests/integration/test_train.py"]
protected_paths: []
extra_paths: ["src/er/cli.py", "src/er/matching/train.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_train.py -q"
branch: "ticket/ER-055-er-train-part-b-s3-artifact"
commit: ""
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-16T14:24:13Z"
session: d27a1aca-d791-47ad-8b50-bc86ed79290b
---
## Description

Gives the trained model a durable home and a selection rule, closing the second half of M8. S4.3.2 fixes the lifecycle: write the settings JSON to `{storage.model_uri_prefix}model_v{N}.json` first, insert the `model_registry` row second, allocate `model_version` as zero-padded `max+1` inside the insert transaction, and set the previous active row to `superseded` in the same transaction. This ticket also wires `er train` per the S4.0 command row (including `--if-changed` → exit 10) and records the stage in `run_stages` per S5.2.

## Scope

### In scope

- `src/er/lake/model_registry.py`: version allocation, object-then-row write ordering, active/superseded pointer flip in one transaction, `active_model()` selection, `load_model_settings(model_version)` reading the artifact back from `params_path`
- `er train` CLI command: `--if-changed` (default false), exit `0` / `10` / `1` / `2` per S4.0; stdout line `model_version, params_path, tf_snapshot_id, corpus_snapshot, metrics`
- `corpus_snapshot` — the DuckLake snapshot version the model was trained against, captured before training begins
- Writing `metrics JSON` containing the verbatim `training:` block plus fitted m/u values, and `config_hash` on the registry row
- One `run_stages` row with `stage='train'`, snapshot range, `duration_ms`, and a `counters` JSON carrying `model_version`, `tf_snapshot_id`, `corpus_snapshot`; exactly one JSON line on stderr
- Invoking `materialize_tf_lookup` under the freshly minted `tf_snapshot_id` and recording `tf_snapshot_id` + `tf_tables_path` on the registry row

### Out of scope

- The Splink call sequence itself (ER-054)
- The activation guard that fails `er reconcile` on a mixed `model_version` edge set (ER-085)
- Committing a fixture model or the regeneration script (ER-056)
- Scoring or `match_scores` (ER-058)
- `er run-all` ever invoking train — S4.0 states it NEVER trains

## Design decisions applied

Closes M8. Naming decision the implementer must honour: ER-017 already owns `src/er/lake/model.py` for the TableSpec registry, so the model-artifact/registry code lands in `src/er/lake/model_registry.py`; S3's `lake/model.py` annotation is satisfied by that split. Constraints easy to miss: (1) **object first, registry row second** — a registry row must never point at a missing object, so a failed upload leaves zero rows; (2) `storage.model_uri_prefix` already ends in `/` and already names the tenant (V14, S6.1) — do not interpolate the tenant a second time; (3) `model_version` is allocated inside the insert transaction, not read-then-write outside it; (4) DuckLake enforces nothing (S5.0), so `at most one status='active' row` is maintained by the writer and asserted by test, never by a constraint; (5) `er train` allocates a new version on **every** invocation unless `--if-changed` is passed (S4 idempotency table). Import consumed symbols from INTERFACES.md; where a path differs, INTERFACES.md wins.

## Acceptance criteria

- [ ] AC1: With the object-store put patched to raise, `er train` exits 1 and `select count(*) from lake.main.model_registry` is unchanged — no row points at a missing object
- [ ] AC2: Three successive `er train` invocations produce `model_version` values `v0001`, `v0002`, `v0003`; after the third, exactly one row has `status='active'` (it is `v0003`) and the other two are `superseded`
- [ ] AC3: `model_registry.params_path` is an `s3://` URI beginning with `storage.model_uri_prefix` and ending `model_v0001.json`, the tenant segment appears exactly once, and fetching that object yields JSON equal to the settings `train_model` produced
- [ ] AC4: `model_registry.corpus_snapshot` equals the lake snapshot version captured immediately before training, and `config_hash` equals the run's `config_hash`; `metrics->'training'` equals the `training:` block of `configs/test.yaml` verbatim
- [ ] AC5: `er train --if-changed` exits 10 and allocates no new `model_version` when the active row already carries this `(config_hash, corpus_snapshot)`; the same invocation without the flag allocates a new one
- [ ] AC6: After a successful `er train`, exactly one `run_stages` row exists with `stage='train'` and `status='succeeded'`, non-null `snapshot_start`/`snapshot_end` and a `counters` JSON containing `model_version`, `tf_snapshot_id` and `corpus_snapshot`; stderr carries exactly one JSON line for the stage
- [ ] AC7: `lake.main.tf_lookup` holds rows for the new `(model_version, tf_snapshot_id)` and the registry row's `tf_snapshot_id` / `tf_tables_path` resolve back to them via `parse_tf_tables_path`
- [ ] AC8: `active_model()` on a lake with no rows raises the precondition error the CLI maps to exit 3

## Tests

- tests/integration/test_train.py::test_object_written_before_registry_row
- tests/integration/test_train.py::test_model_version_allocation_is_zero_padded_max_plus_one
- tests/integration/test_train.py::test_activation_supersedes_previous_in_one_transaction
- tests/integration/test_train.py::test_params_path_is_prefixed_once_and_round_trips
- tests/integration/test_train.py::test_corpus_snapshot_and_config_hash_recorded
- tests/integration/test_train.py::test_if_changed_exits_10_without_allocating
- tests/integration/test_train.py::test_train_writes_run_stage_and_one_json_log_line
- tests/integration/test_train.py::test_tf_lookup_and_tf_tables_path_agree

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_train.py -q
uv run mypy --strict src/er/lake/model_registry.py src/er/matching/train.py
```

## Definition of Done

- Acceptance criteria met and the verify command passes
- Artifact upload precedes the registry insert in code order, and the failure path is tested, not argued
- `at most one active row` asserted by an integration test over `model_registry`
- `er train` documented in the CLI help with the S4.0 flags and exit codes
- `mypy --strict` clean on the touched modules
- `provides` entries recorded in INTERFACES.md
- Committed on a branch off main
