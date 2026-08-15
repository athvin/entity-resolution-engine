---
id: ER-032
title: "Deletion ingest arm: is_deleted/deleted_at, tombstone content_hash sentinel, --full-refresh-keys, empty-delivery guard, resurrection"
milestone: M1
status: in_progress
kind: code
size: M
gates: full
depends_on: ["ER-028", "ER-031"]
spec_refs: ["s4-0", "s4-1", "s4-1-1", "s5", "s5-0", "s5-2", "s8-2-1", "s12-1"]
gap_refs: ["M15", "D8", "NEW-Deletion"]
provides: ["src/er/ingest/landing.py::TOMBSTONE_CONTENT_HASH", "src/er/ingest/landing.py::derive_tombstones", "src/er/ingest/landing.py::count_resurrected", "src/er/cli.py::ingest --full-refresh-keys", "tests/integration/data/ingest_deletion/", "fixture:ingest_deletion"]
consumes: ["src/er/ingest/landing.py::ingest_delivery", "src/er/ingest/landing.py::IngestManifest", "src/er/ingest/hashing.py::content_hash", "tests/helpers/expected.py::load_scenario", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env", "relation:raw_records", "relation:ingest_batches", "RunContext", "src/er/errors.py"]
owns: ["tests/integration/test_ingest_deletion.py", "tests/unit/test_tombstone_hash.py", "tests/integration/data/ingest_deletion"]
protected_paths: ["tests/integration/test_ingest_idempotence.py"]
extra_paths: ["src/er/ingest/landing.py", "src/er/cli.py"]
attempts: 1
verify: "bash scripts/ci/itest.sh tests/integration/test_ingest_deletion.py -q && uv run pytest tests/unit/test_tombstone_hash.py -q"
branch: "ticket/ER-032-deletion-ingest-arm-is-deleted-deleted"
commit: ""
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T16:40:31Z"
session: 8df1d201-459c-4f64-95d6-62605feccd0b
---
## Description

Add the S4.1.1 deletion arm to `er ingest`: `--full-refresh-keys` treats a delivery as the complete key set for a source and appends a tombstone version row (`is_deleted=true`, `deleted_at` = the batch's `ingested_at`, sentinel `content_hash`) for every live key absent from it. Ship the empty-delivery guard that refuses to tombstone (exit `2`, not `10`) and the resurrection path in which a re-appearing key is an ordinary content version. This is what makes D8's `member_removed` / `retired` states reachable downstream and what the ingest half of T-DEL-1a asserts.

## Scope

### In scope

- `--full-refresh-keys` flag on `er ingest` and tombstone derivation against the currently-live key set for that source only.
- `TOMBSTONE_CONTENT_HASH = '0' * 64` defined once, plus proof it is unreachable from the hash function.
- `deleted_at` = the batch's `ingested_at` on tombstone rows; NULL on every other row.
- The S4.1.1 empty-delivery guard with its literal message and exit `2`.
- `tombstone_count` and `resurrected_count` on the `ingest_batches` row, in the `run_stages` counters payload and on the stdout manifest line.
- Resurrection accounting: a live key in an ordinary delivery whose current version is a tombstone counts as `resurrected` (and as `changed`, because the key was previously seen).
- An ingest-level fixture with `base/`, `refresh/` and `resurrect/` phase directories under this ticket's own data root, loaded through the ER-028 machinery.

### Out of scope

- Everything T-DEL-1a asserts about `int_std_records` (tombstone exclusion after `er standardize`) — `int_std_records` does not exist until M2.
- Edge invalidation, `member_removed` / `split` / `retired` emission and the retraction path — ER-083.
- The committed `fixtures/static/deletion_scenario/` scenario and its `expected/` files — ER-083 owns that path; this ticket must not create it.
- Any change that weakens the ER-031 assertions in `tests/integration/test_ingest_idempotence.py`.

## Design decisions applied

Implements D8 and gap entries M15 / NEW-Deletion. Two constraints an implementer will otherwise get wrong. (1) **`payload` on a tombstone.** S4.1.1 says `payload = NULL` but S5 declares `payload JSON NOT NULL` and DuckLake enforces NOT NULL (S5.0). The reconciling reading — and the one this ticket implements — is the JSON *null document* (`'null'::JSON`), which satisfies the NOT NULL constraint while carrying no source row: assert `json_type(payload) = 'NULL'` and never a SQL NULL. Changing the column to nullable would be a spec amendment and is out of scope. (2) **The empty-delivery guard fires before any write.** It is a refusal to destroy, so exit `2` (a validation error that aborts an `er run-all` chain), not `10`; no `raw_records` row and no `ingest_batches` row is persisted by a refused invocation, and the message is the literal `empty full-refresh delivery: refusing to tombstone <n> live keys for source <S>`. Also: a full-refresh delivery is scoped to one `source_system` — keys of other sources are never tombstoned; and tombstoning is itself append-only and idempotent, because a key whose current version is already a tombstone is not live.

## Acceptance criteria

- [ ] AC1: Given a `base/` delivery of N keys for `crm`, `er ingest --source crm --path refresh/ --full-refresh-keys` over a delivery omitting exactly two of those keys exits `0`, appends exactly two rows with `is_deleted=true`, `content_hash = repeat('0',64)`, `json_type(payload) = 'NULL'` and `deleted_at` equal to that batch's `ingested_at`, and appends no tombstone for any key present in the delivery.
- [ ] AC2: The resulting `ingest_batches` row has `tombstone_count=2`, `full_refresh_keys=true` and `resurrected_count=0`; keys present in both deliveries with unchanged content are counted in `unchanged_count`.
- [ ] AC3: Re-running the same `--full-refresh-keys` delivery a second time exits `10` and writes an `ingest_batches` row with `tombstone_count=0`, because the two omitted keys' current versions are already tombstones (no duplicate tombstone rows exist for any key).
- [ ] AC4: A `--full-refresh-keys` delivery that parses to zero records exits `2`, emits the literal message `empty full-refresh delivery: refusing to tombstone <n> live keys for source <S>` with `<n>` equal to the live key count, and leaves `raw_records` and `ingest_batches` row counts unchanged.
- [ ] AC5: An ordinary (non-`--full-refresh-keys`) `resurrect/` delivery re-appearing exactly one tombstoned key exits `0`, appends one row with `is_deleted=false` and an `ingested_at` strictly greater than that key's tombstone row, and reports `resurrected_count=1`, `changed_count=1`, `new_count=0`, `tombstone_count=0`.
- [ ] AC6: `TOMBSTONE_CONTENT_HASH` is defined exactly once and `content_hash(row, columns) != TOMBSTONE_CONTENT_HASH` holds for every row generated by the unit property test, including all-NULL and all-empty-string rows.
- [ ] AC7: Tombstones for source `crm` leave every live key of `billing` and `webforms` untouched (row counts and current versions unchanged).

## Tests

- tests/integration/test_ingest_deletion.py::test_full_refresh_keys_appends_tombstones_for_omitted_keys
- tests/integration/test_ingest_deletion.py::test_repeat_full_refresh_tombstones_nothing_and_exits_10
- tests/integration/test_ingest_deletion.py::test_empty_full_refresh_delivery_is_refused_with_exit_2
- tests/integration/test_ingest_deletion.py::test_resurrection_appends_ordinary_version_and_counts_it
- tests/integration/test_ingest_deletion.py::test_full_refresh_is_scoped_to_one_source
- tests/unit/test_tombstone_hash.py::test_sentinel_is_unreachable_from_content_hash
- tests/unit/test_tombstone_hash.py::test_sentinel_is_defined_once

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_ingest_deletion.py -q && uv run pytest tests/unit/test_tombstone_hash.py -q
bash scripts/ci/itest.sh tests/integration/test_ingest_idempotence.py -q
uv run mypy --strict src/er
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- All acceptance criteria have a named passing test
- Verify command passes; ER-031's integration test still passes unmodified
- Tombstone rows carry the JSON null document, never a SQL NULL `payload`
- The empty-delivery guard writes nothing at all and exits `2`
- `tombstone_count` / `resurrected_count` appear on `ingest_batches`, in `run_stages.counters` and on the stdout manifest line
- No path under `fixtures/static/deletion_scenario/` is created
- mypy --strict and ruff clean
