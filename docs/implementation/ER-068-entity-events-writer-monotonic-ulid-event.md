---
id: ER-068
title: "entity_events writer: monotonic ULID event_id, seq, per-type details schema, details_hash within-run idempotency key (unit)"
milestone: M3
status: in_progress
kind: code
size: M
gates: fast
depends_on: ["ER-013", "ER-017", "ER-023"]
spec_refs: ["s5", "s5-0", "s4-5-3", "s4-5-4", "s4-5-5", "s4-4-2", "s4-1-1", "s4-0"]
gap_refs: ["MINOR-event_id", "M3"]
provides: ["src/er/entities/events.py::Event", "src/er/entities/events.py::EventLog", "src/er/entities/events.py::EVENT_TYPES", "src/er/entities/events.py::EVENT_DETAILS_SCHEMA", "src/er/entities/events.py::canonical_details", "src/er/entities/events.py::details_hash", "src/er/entities/events.py::append_events"]
consumes: ["src/er/entities/ids.py::UlidFactory", "src/er/entities/ids.py::MonotonicUlidFactory", "src/er/entities/ids.py::CountingIdFactory", "src/er/entities/ids.py::IdFactory", "src/er/lake/columns.py::VOLATILE_COLUMNS", "src/er/lake/model.py::TABLE_SPECS", "src/er/obs/run_context.py::RunContext", "relation:entity_events"]
owns: ["src/er/entities/events.py", "tests/unit/entities/__init__.py", "tests/unit/entities/test_events.py"]
protected_paths: []
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/entities/test_events.py -q"
branch: "ticket/ER-068-entity-events-writer-monotonic-ulid-event"
commit: ""
spec_sha: "2e62460d9f41a842"
updated_at: "2026-08-17T05:56:24Z"
session: 6a729e8f-bed3-4d4c-9540-98b3386843be
---
## Description

`entity_events` is the replay log `entity_membership` history lives in (D3), and S5 gives it `event_id`, `seq`, `details` and `details_hash` with the idempotency key `(run_id, entity_id, event_type, details_hash)` from S4.5.4. DuckLake has no sequences, so `event_id` is a monotonic ULID minted in Python and `seq` is dense and 1-based per `run_id`; without a defined per-type `details` document and a canonicalisation rule the hash is not reproducible and a re-run appends duplicate events, which is what makes T-PERM-1's 'exactly one merged event' unassertable. This ticket ships the writer as a pure accumulator plus a thin flush, unit-tested with an injectable `IdFactory`.

## Scope

### In scope

- `EVENT_TYPES` = `{created, member_added, member_removed, merged, split, retired, edge_cut}` as one definition, matching the S5 enum
- `EVENT_DETAILS_SCHEMA`: the required keys per event type, with record-key lists sorted ascending and an optional `reason` field carrying `correction_pass` when the run is a correction pass
- `canonical_details` (sorted keys, no aliases, UTF-8, compact separators) and `details_hash` = SHA-256 hex of it
- `EventLog`: an in-memory accumulator that mints `event_id` through the injected `IdFactory`, assigns dense 1-based `seq` per `run_id` in emission order, and collapses a re-emission of an identical `(run_id, entity_id, event_type, details_hash)` to a no-op
- `append_events(conn, events)` writing the accumulated rows in one statement, with `occurred_at` supplied by the writer

### Out of scope

- Deciding WHICH events a reconcile produces — ER-073 (plan) and ER-074 (apply) own that
- Event replay into `entity_membership` (ER-080)
- The `er_touched_entities` formula over this run's events (ER-092)
- Any lake integration test: this ticket's verify is unit-only and the flush is exercised by the reconcile-apply ticket

## Design decisions applied

Implements the MINOR `event_id` gap entry and M3's replay half under D3. Constraints: (1) `event_id`, `seq`, `occurred_at` and `run_id` are all `VOLATILE_COLUMNS` members (S5.0) — the accumulator must never fold them into `details` or into `details_hash`, or every determinism comparison breaks; (2) the idempotency key is `(run_id, entity_id, event_type, details_hash)` and NOT `event_id`, so a re-run producing identical output writes zero events (S4.5.4); (3) `seq` is dense and 1-based per `run_id`, so a collapsed duplicate must not consume a `seq` value; (4) replay order is `(occurred_at, seq)` (S4.5.3), which is why `seq` is assigned in emission order rather than sorted afterwards; (5) `details` for `member_removed` carries a cause drawn from `{tombstone, supersession, recluster}` (S4.1.1, S4.5.5) and `edge_cut` carries `rec_a_key`, `rec_b_key`, `match_probability`, `assertion_id` and `cut_id` (S4.4.2) — these are the payloads downstream tests assert on.

## Acceptance criteria

- [ ] AC1: `details_hash` is invariant under key reordering and whitespace in the input mapping, and differs for any value change; two `EventLog`s built from the same event sequence produce identical `details_hash` values across processes
- [ ] AC2: Emitting n distinct events under one `run_id` yields `seq` exactly `1..n` in emission order with no gaps, and `event_id` values that are strictly increasing as strings (monotonic ULID)
- [ ] AC3: Re-emitting an event with the same `(run_id, entity_id, event_type, details_hash)` returns the already-recorded event, adds no row, and does not advance `seq`
- [ ] AC4: Every event type in `EVENT_TYPES` has an entry in `EVENT_DETAILS_SCHEMA`, and constructing an event with a missing required key or an unknown key raises; `EVENT_TYPES` equals the `entity_events.event_type` domain in the TableSpec registry
- [ ] AC5: Record-key lists inside `details` are stored ascending regardless of input order, so two orderings of the same membership change hash identically
- [ ] AC6: No `VOLATILE_COLUMNS` member appears inside any `details` document, asserted over every schema entry
- [ ] AC7: An `EventLog` built with `CountingIdFactory` produces byte-identical rows modulo `event_id`/`occurred_at` across two runs of the same input (the S4.5.4 D2 property)

## Tests

- tests/unit/entities/test_events.py::test_details_hash_is_canonical_and_stable
- tests/unit/entities/test_events.py::test_seq_is_dense_and_one_based
- tests/unit/entities/test_events.py::test_duplicate_idempotency_key_is_a_noop
- tests/unit/entities/test_events.py::test_details_schema_covers_every_event_type
- tests/unit/entities/test_events.py::test_record_key_lists_are_sorted
- tests/unit/entities/test_events.py::test_no_volatile_column_inside_details
- tests/unit/entities/test_events.py::test_output_is_byte_identical_modulo_minted_ids

## Verification

```bash
uv run pytest tests/unit/entities/test_events.py -q
uv run mypy --strict src/er/entities/events.py
uv run ruff check src/er tests && uv run ruff format --check src/er tests
```

## Definition of Done

- `EVENT_TYPES` and the `entity_events` enum in the TableSpec registry are asserted equal by a test, not by inspection
- `EventLog` is constructible and fully testable with no lake connection
- `details_hash` has exactly one implementation and it is used by both the accumulator and any future reader
- `bash scripts/gates.sh` green; INTERFACES entry lists `EventLog`, `details_hash`, `EVENT_DETAILS_SCHEMA`
