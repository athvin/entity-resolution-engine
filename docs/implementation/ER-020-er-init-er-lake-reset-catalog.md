---
id: ER-020
title: "er init + er lake reset + catalog-init compose service (+ compose contract test update)"
milestone: M1
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-014", "ER-019"]
spec_refs: ["s4-0", "s4-0b", "s5-1", "s7-1", "s7-4", "s8-1"]
gap_refs: ["M19", "B1", "B2", "M22"]
provides: ["src/er/lake/init.py::init_lake", "src/er/lake/init.py::reset_lake", "src/er/lake/init.py::DataPathMismatchError", "src/er/lake/init.py::TenantMismatchError", "src/er/cli.py::init", "src/er/cli.py::lake_reset", "tests/conftest.py::initialised_lake", "docker/compose.yaml::catalog-init", "tests/integration/test_init.py"]
consumes: ["src/er/cli.py::app", "src/er/cli.py::emit_stage_line", "src/er/errors.py::ExitCode", "src/er/lake/ddl.py::apply", "src/er/lake/ducklake.py::connect", "src/er/lake/catalog.py::read_data_path", "src/er/lake/catalog.py::drop_metadata_schema", "src/er/lake/catalog.py::advisory_lock", "src/er/lake/objectstore.py::ObjectStore.delete_prefix", "src/er/config/loader.py::load_config", "tests/conftest.py::lake_ns", "tests/conftest.py::er_env", "scripts/ci/itest.sh"]
owns: ["src/er/lake/init.py", "tests/integration/test_init.py"]
protected_paths: ["src/er/lake/ddl.py", "src/er/lake/model.py"]
extra_paths: ["src/er/cli.py", "docker/compose.yaml", "tests/unit/test_compose_contract.py", "tests/conftest.py"]
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_init.py -q && uv run pytest tests/unit/test_compose_contract.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Turn the lake bootstrap into a real CLI verb and a Compose service: `er init` installs/loads extensions, creates the secret, ATTACHes, applies the `ddl.py`-owned relations and DETACHes, idempotently; `er lake reset --confirm-tenant NAME` is the only escape from the immutable `DATA_PATH`. M19's finding is that nothing anywhere invoked `ddl.py`, so M1's exit criterion was unreachable; B1's fix adds a `catalog-init` one-shot mirroring `objectstore-init`, which `pipeline` and `benchmark` gate on with `service_completed_successfully`. This is also where the S8.1 fixture finally gets an initialised namespace instead of an empty one.

## Scope

### In scope

- `er init [--force]`: attach, `ddl.apply`, detach; stdout one line per relation `created`/`exists`, `--json` emitting `{relation, action}` objects
- `DATA_PATH` immutability: compare the catalog's recorded value with `$ER_LAKE_DATA_PATH` and exit 3 with the literal S4.0 message when they differ
- `er lake reset --confirm-tenant NAME`: exit 2 on a tenant mismatch; otherwise take the tenant advisory lock, drop the catalog metadata schema and delete the `DATA_PATH` prefix; stdout `dropped_schema, deleted_prefix`; exit 3 when the lock is not acquired
- `catalog-init` service in `docker/compose.yaml`: `<<: *er-image`, `environment: *er-env`, `command: ["er","init"]`, `restart: "no"`, no `profiles:` key, `depends_on` catalog `service_healthy` + `objectstore-init` `service_completed_successfully`; `pipeline` and `benchmark` gate on it
- Extend `tests/unit/test_compose_contract.py` with the `catalog-init` assertions
- An `initialised_lake` fixture in `tests/conftest.py` layering `er init` onto the ER-018 namespace (S8.1 step 3)

### Out of scope

- Persisting `runs` / `run_stages` rows for `init` and `reset` — ER-023 owns `RunContext`; this ticket only returns the correct exit codes and stdout
- Generalising the advisory lock onto every mutating command and the `writer lock held for tenant …` message (ER-024)
- `er doctor` and its S2.1 assertion list (ER-022); lake maintenance (ER-025)
- Creating dbt-owned relations or running dbt at all: `er init` creates `ddl.py`-owned relations only

## Design decisions applied

Closes M19 (`er init` exists and is what creates `ddl.py`-owned tables), B1 (`catalog-init` gates the pipeline), B2 (the registry is applied by a real verb) and M22 (the harness now initialises its namespace). Easy to miss: (1) the mismatch message is literal and must match S4.0 character for character, including the `er lake reset --confirm-tenant <tenant>` remedy clause; (2) `er lake reset` is a writer like any other — it takes the same advisory lock every writer takes (S4.0b) and exits 3 when it cannot; (3) `--confirm-tenant` is compared against `tenant` in the validated config, not against the metadata schema, so a typo can never reach the destructive path (exit 2); (4) `er init` creates only `ddl.py`-owned relations; the dbt-owned relations appear on the first `dbt run` (S8.1); (5) `catalog-init` declares NO `profiles:` key so it is enabled under every profile, and the stack is driven with `run`, never `up --abort-on-container-exit` (S7.4); (6) prefix deletion goes through `ObjectStore.delete_prefix`, whose root guard must not be bypassed.

## Acceptance criteria

- [ ] AC1: On an empty namespace `er init` exits 0 and prints exactly fourteen `created` lines; a second `er init` exits 0 printing fourteen `exists` lines and creating nothing; with `--json` each stdout line parses as an object with exactly the keys `relation` and `action`.
- [ ] AC2: After a successful init, re-running `er init` with a different `ER_LAKE_DATA_PATH` exits 3 with exactly `lake DATA_PATH immutable: catalog=<a> env=<b>; use 'er lake reset --confirm-tenant <tenant>' to destroy and recreate this namespace` and changes no relation.
- [ ] AC3: `er lake reset --confirm-tenant wrong` exits 2 and leaves the schema and every object under the prefix intact.
- [ ] AC4: `er lake reset --confirm-tenant test` exits 0, prints `dropped_schema` and `deleted_prefix`, and afterwards the catalog has no `er_test_<ns>` schema and the object store lists zero keys under `s3://lake/test/<ns>/`; a following `er init` re-creates all fourteen relations and exits 0.
- [ ] AC5: With the tenant advisory lock held by a second connection, `er lake reset --confirm-tenant test` exits 3 and destroys nothing.
- [ ] AC6: `tests/unit/test_compose_contract.py` asserts the `catalog-init` service exists with `command: ["er","init"]`, `restart: "no"`, no `profiles:` key, and `depends_on` of catalog `service_healthy` plus `objectstore-init` `service_completed_successfully`, and that both `pipeline` and `benchmark` depend on `catalog-init` with `service_completed_successfully`.
- [ ] AC7: The `initialised_lake` fixture yields a namespace holding exactly the fourteen `ddl.py`-owned relations and zero dbt-owned relations.

## Tests

- tests/integration/test_init.py::test_init_creates_then_reports_exists
- tests/integration/test_init.py::test_init_json_output_shape
- tests/integration/test_init.py::test_data_path_mismatch_exits_3_with_literal_message
- tests/integration/test_init.py::test_reset_rejects_tenant_mismatch
- tests/integration/test_init.py::test_reset_drops_schema_and_prefix_then_init_recreates
- tests/integration/test_init.py::test_reset_exits_3_when_writer_lock_held
- tests/integration/test_init.py::test_initialised_lake_fixture_has_only_ddl_owned_relations
- tests/unit/test_compose_contract.py::test_catalog_init_service_contract
- tests/unit/test_compose_contract.py::test_pipeline_and_benchmark_gate_on_catalog_init

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_init.py -q && uv run pytest tests/unit/test_compose_contract.py -q
bash scripts/ci/compose_smoke.sh
uv run mypy --strict src/er/lake/init.py src/er/cli.py
```

## Definition of Done

- `er init` idempotent, correct stdout in both human and `--json` modes, exit 3 on `DATA_PATH` drift with the literal message
- `er lake reset` gated by `--confirm-tenant` (exit 2) and the tenant advisory lock (exit 3); drops schema and prefix
- `catalog-init` service added and gated on by `pipeline` and `benchmark`; compose contract test extended
- `initialised_lake` fixture layered onto the ER-018 namespace without altering its teardown
- `ddl.py` and `model.py` unmodified — init emits no DDL of its own
- Verify command passes; compose smoke green
