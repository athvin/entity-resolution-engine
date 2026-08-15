---
id: ER-026
title: "Test harness B: function isolation, snapshot-literal collection guard, --keep-lake, xdist ns derivation, teardown-on-failure, sub-namespace factory (for T-INC-1)"
milestone: M1
status: done
kind: code
size: M
gates: full
depends_on: ["ER-020", "ER-024"]
spec_refs: ["s4", "s5", "s5-0", "s8-1", "s8-3"]
gap_refs: ["M22", "M7"]
provides: ["tests/conftest.py::clean_lake", "tests/conftest.py::sub_namespace", "tests/conftest.py::derive_namespace", "tests/conftest.py::pytest_addoption --keep-lake", "tests/conftest.py::pytest_collection_modifyitems snapshot-literal guard"]
consumes: ["tests/conftest.py::lake_ns", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env", "src/er/lake/model.py::TABLES", "src/er/lake/catalog.py::catalog_connection", "src/er/lake/objectstore.py::S3Client", "src/er/cli.py::app", "src/er/entities/ids.py::UlidFactory"]
owns: ["tests/integration/test_harness_isolation.py", "tests/unit/test_namespace_derivation.py"]
protected_paths: ["tests/integration/test_harness_namespace.py"]
extra_paths: ["tests/conftest.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_harness_isolation.py -q"
branch: "ticket/ER-026-test-harness-b-function-isolation-snapshot"
commit: "47634326e5193b690cd899fae5b3115c9796d56c"
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T16:05:51Z"
session: 3d18930e-be66-4693-bf9d-829670e87edb
---
## Description

S8.1's isolation contract has two halves: the session namespace (ER-018) and the per-test isolation that keeps test ordering from becoming load-bearing. This ticket ships the second half — a function-scoped fixture that DELETEs every `ddl.py`-owned relation and drops the dbt-owned ones, the `sub_namespace` factory that T-INC-1 needs to build two independent universes inside one session, xdist-suffixed namespace derivation, `--keep-lake` for debugging, teardown under `try/finally` so a failing test still reclaims its namespace, and a collection-time guard enforcing S8.1's blanket rule that no test may reference an absolute snapshot version. Closes the rest of M22 and the determinism-comparison half of M7.

## Scope

### In scope

- `clean_lake`: function-scoped autouse fixture deleting from every `ddl.py`-owned relation and dropping every dbt-owned relation between tests, with the relation list derived from the TableSpec registry rather than hard-coded.
- `sub_namespace`: a factory fixture repeating S8.1 steps 1–4 under `er_test_<ns>_a` / `er_test_<ns>_b`, each with its own `METADATA_SCHEMA` and `DATA_PATH`, `er init`-ed on entry and fully reclaimed on exit.
- `derive_namespace(ulid, worker)`: `ns = f"{ulid}_{worker}"` when `PYTEST_XDIST_WORKER` is set, plain ULID otherwise — a pure function with its own unit test.
- `--keep-lake`: suppresses teardown and prints the retained namespace to stdout.
- Teardown-on-failure: expiry, cleanup, S3 prefix delete, `DETACH`, `DROP SCHEMA … CASCADE` inside `try/finally` for both session and sub-namespaces.
- A collection-time guard over `tests/integration/**` that fails collection, naming path and line, when a test source contains an absolute snapshot version (an integer literal inside `AT (VERSION => …)`, or a comparison of `snapshot_start`/`snapshot_end` against an integer literal).

### Out of scope

- The session-scoped namespace fixture itself (ER-018) — `tests/integration/test_harness_namespace.py` is protected and must stay green unmodified.
- The T-INV-1 autouse invariant finalizer, which lives in `tests/integration/conftest.py` (ER-072).
- Scenario loading and expected-file comparison (ER-027, ER-028).
- Any assertion on snapshot **counts**: a stage commits a range, and counts are not assertable at all (S4 preamble).

## Design decisions applied

M22 + M7. Constraints: (1) the delete list MUST be derived from the registry — a hard-coded list of fourteen relations silently rots the first time S5 grows, and the test asserts the derivation. (2) Integration tests run single-process; `-n auto` is a unit-layer option only, so the xdist suffix exists for the unit layer and for nothing else. (3) Teardown runs under `try/finally`, because a namespace leaked by a failing test is charged to the next run's catalog. (4) The snapshot-literal guard runs at **collection** time, not as a test, so a violating file fails fast and names itself; it is the mechanical form of S8.1's blanket rule and it must have a self-test proving both arms (a literal fails, a runtime-captured variable collects). (5) `sub_namespace` is the only sanctioned way to build a second universe: two universes inside one namespace would share `runs`/`run_stages` and make T-INC-1 meaningless.

## Acceptance criteria

- [ ] AC1: A test that writes at least one row into every `ddl.py`-owned relation is followed by a test asserting all of them are empty; the fixture's relation list equals `{spec.name for spec in TABLES if spec.owner == 'ddl'}`, asserted directly.
- [ ] AC2: A test that creates a dbt-owned relation is followed by a test that finds it absent from `lake.main`.
- [ ] AC3: `sub_namespace` yields two namespaces with distinct `METADATA_SCHEMA` and `DATA_PATH`; writing rows in A leaves B empty, and after the test both catalog schemas are gone and both S3 prefixes list zero objects.
- [ ] AC4: A deliberately failing test using `sub_namespace` still leaves zero `er_test_%_a`/`_b` schemas for that namespace, asserted from the catalog by a following test.
- [ ] AC5: Running the suite with `--keep-lake` leaves the namespace present and prints it to stdout; running it without leaves the catalog schema dropped — asserted by querying the catalog after two subprocess pytest runs.
- [ ] AC6: `derive_namespace(ulid, 'gw3')` ends with `_gw3` and `derive_namespace(ulid, None)` does not; two distinct workers never produce the same namespace for the same ULID.
- [ ] AC7: Collecting a file containing `AT (VERSION => 118)` fails with the path and line reported; the same file rewritten to bind a runtime-captured `:snap` collects cleanly.

## Tests

- tests/unit/test_namespace_derivation.py::test_xdist_worker_suffix_applied
- tests/unit/test_namespace_derivation.py::test_namespaces_are_unique_per_worker
- tests/integration/test_harness_isolation.py::test_function_isolation_empties_every_ddl_owned_relation
- tests/integration/test_harness_isolation.py::test_delete_list_is_derived_from_registry
- tests/integration/test_harness_isolation.py::test_dbt_owned_relations_are_dropped_between_tests
- tests/integration/test_harness_isolation.py::test_sub_namespaces_are_independent_and_reclaimed
- tests/integration/test_harness_isolation.py::test_teardown_runs_after_a_failing_test
- tests/integration/test_harness_isolation.py::test_keep_lake_suppresses_teardown
- tests/integration/test_harness_isolation.py::test_snapshot_literal_guard_fails_collection

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_harness_isolation.py -q
bash scripts/ci/itest.sh tests/integration/test_harness_namespace.py -q
uv run pytest tests/unit/test_namespace_derivation.py -q
```

## Definition of Done

- All acceptance criteria demonstrated by the listed node ids
- Function isolation derived from the TableSpec registry, not a hard-coded relation list
- `sub_namespace` available and proven independent, ready for T-INC-1 (ER-093)
- Teardown proven to run on test failure for both session and sub-namespaces
- Snapshot-literal collection guard active over `tests/integration/**` with both arms self-tested
- `tests/integration/test_harness_namespace.py` unmodified and still green
- ruff clean; verify command passes
