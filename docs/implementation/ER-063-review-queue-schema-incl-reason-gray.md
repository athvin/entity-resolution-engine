---
id: ER-063
title: "review_queue: schema incl. reason ∈ {gray_band, never_unsatisfiable, coherence}, idempotent upsert, er review list|resolve, resolution→assertion in one transaction"
milestone: M3
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-062"]
spec_refs: ["s4-3-5", "s4-4", "s4-4-2", "s4-0", "s5", "s5-0", "s11", "s8-3"]
gap_refs: ["M20", "M19", "D5"]
provides: ["src/er/review/queue.py::ReviewRow", "src/er/review/queue.py::upsert_gray_band_pairs", "src/er/review/queue.py::upsert_escalation", "src/er/review/queue.py::upsert_entity_finding", "src/er/review/queue.py::open_reviews", "src/er/review/queue.py::resolve_review", "src/er/review/queue.py::REVIEW_REASONS", "src/er/review/queue.py::REVIEW_STATUSES", "cli:er review list", "cli:er review resolve", "relation:review_queue"]
consumes: ["src/er/review/assertions.py::add_assertion", "src/er/review/assertions.py::Assertion", "src/er/entities/ids.py::canonicalize_pair", "src/er/entities/ids.py::UlidFactory", "src/er/errors.py::ConfigError", "src/er/errors.py::StageFailure", "src/er/cli.py::app", "src/er/lake/ducklake.py::connect", "src/er/obs/run_context.py::RunContext", "tests/conftest.py::lake_conn", "tests/conftest.py::er_env", "relation:review_queue", "relation:assertions"]
owns: ["src/er/review/queue.py", "tests/unit/review/test_queue_upsert.py", "tests/integration/test_review_cli.py"]
protected_paths: []
extra_paths: ["src/er/cli.py"]
attempts: 0
verify: "uv run pytest tests/unit/review/test_queue_upsert.py -q && bash scripts/ci/itest.sh tests/integration/test_review_cli.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

S4.3.5 requires gray-band pairs to land in `review_queue` as an idempotent upsert that refreshes `last_seen_run_id` and skips already-resolved subjects, S4.4.2 routes `never_unsatisfiable` escalations into the same table, and S11 routes coherence findings there with `subject_type='entity'`. S5.0 makes `reason` part of the open-row logical key precisely so one pair can be open for two independent reasons. This ticket ships `src/er/review/queue.py`, the `er review list|resolve` verbs of S4.0, and the rule that resolving to `match`/`no_match` writes the corresponding `assertions` row in the same transaction — the mechanism that makes steward corrections survive re-runs.

## Scope

### In scope

- `upsert_gray_band_pairs`, `upsert_escalation` (`reason='never_unsatisfiable'`) and `upsert_entity_finding` (`reason='coherence'`), all obeying the same insert/refresh/skip rule
- The open-row key `(subject_type, rec_a_key, rec_b_key, entity_id, reason)` — two reasons for one pair are two rows
- `waterfall` payload retention: the `gamma_*` comparison vector and per-comparison Bayes factors, stored, not projected away
- `er review list [--status open] [--limit 100]` and `er review resolve --review-id ID --as match|no_match|dismiss --by USER` with the S4.0 exit codes
- `resolve_review` writing the `always` (match) / `never` (no_match) assertion in the same database transaction as the status update, and nothing for `dismiss`

### Out of scope

- Producing gray-band pairs — `er match` calls `upsert_gray_band_pairs` (ER-058 already owns the call site); this ticket owns the function it calls
- The never-cut algorithm that produces escalations (ER-076) and the coherence scorer that produces entity findings (ER-104)
- T-REVIEW-1 / T-REVIEW-2 end-to-end scenarios (ER-086) — this ticket proves the table mechanics, not the reconcile loop
- Any change to `assertions` write semantics (ER-062 owns them)

## Design decisions applied

Implements gap-report M20 and M19 under D5. Constraints: (1) the skip rule is not 'skip if a row exists' but 'skip if the subject is in `{resolved_match, resolved_no_match, dismissed}`' — a dismissed pair must never resurface and must not have its `last_seen_run_id` bumped either, or a dismissal silently decays; (2) `first_seen_run_id` is written once and never updated; (3) `reason` is in the open-row key, so a pair open for `gray_band` and later escalated as `never_unsatisfiable` yields two rows, not one updated row; (4) the resolution and the assertion insert are one transaction on one connection — the assertion insert may be rejected by ER-062's precedence rule, and when it is, the review row MUST remain `open` (no partial commit); (5) an entity-subject row has NULL `rec_a_key`/`rec_b_key`, so `resolve --as match|no_match` on it has no pair to assert and exits `2`; only `dismiss` is legal there.

## Acceptance criteria

- [ ] AC1: `upsert_gray_band_pairs` on a new pair inserts exactly one row with `subject_type='pair'`, `reason='gray_band'`, `status='open'`, `first_seen_run_id == last_seen_run_id == run_id`, and a `waterfall` object containing every `gamma_*` key and a per-comparison Bayes factor; calling it again under a different `run_id` leaves the row count at 1 and updates only `last_seen_run_id`
- [ ] AC2: A row whose `status` is `resolved_match`, `resolved_no_match` or `dismissed` is skipped by a later upsert: no new row is inserted and its `last_seen_run_id` is byte-unchanged
- [ ] AC3: Upserting the same canonical pair with `reason='never_unsatisfiable'` while its `gray_band` row is open produces two rows, and `dbt test --select tag:keys` stays green (the filtered uniqueness key includes `reason`)
- [ ] AC4: `er review resolve --review-id <id> --as match --by tester` exits `0`, sets `status='resolved_match'` with `resolved_by`/`resolved_at` populated, and writes exactly one `active` `always` assertion for that canonical pair; `--as no_match` writes a `never`; `--as dismiss` writes zero assertion rows
- [ ] AC5: When the assertion insert is rejected (an active opposite-kind assertion already exists), `er review resolve` exits `1`, the `review_queue` row is still `status='open'` with NULL `resolved_at`, and no assertion row was added — asserted by row counts before and after
- [ ] AC6: `er review list --status open --limit 2` prints at most two rows of `review_id, subject_type, keys, match_probability, status` and exits `0`; with no open rows it exits `10`; `er review resolve --review-id <unknown>` exits `2`
- [ ] AC7: `upsert_entity_finding` writes `subject_type='entity'`, `reason='coherence'`, non-NULL `entity_id`, NULL `rec_a_key`/`rec_b_key`, and `er review resolve --as match` against that row exits `2` while `--as dismiss` exits `0`

## Tests

- tests/unit/review/test_queue_upsert.py::test_insert_then_refresh_last_seen_only
- tests/unit/review/test_queue_upsert.py::test_resolved_subject_is_skipped_not_refreshed
- tests/unit/review/test_queue_upsert.py::test_two_reasons_for_one_pair_are_two_rows
- tests/unit/review/test_queue_upsert.py::test_waterfall_retains_gamma_and_bayes_factors
- tests/integration/test_review_cli.py::test_resolve_match_writes_assertion_in_one_transaction
- tests/integration/test_review_cli.py::test_resolve_rollback_leaves_row_open
- tests/integration/test_review_cli.py::test_review_list_exit_codes
- tests/integration/test_review_cli.py::test_entity_subject_resolution_rules

## Verification

```bash
uv run pytest tests/unit/review/test_queue_upsert.py -q
bash scripts/ci/itest.sh tests/integration/test_review_cli.py -q
uv run mypy --strict src/er/review
```

## Definition of Done

- `er review` matches the S4.0 signature exactly — no flags beyond `list [--status] [--limit]` and `resolve --review-id --as --by`
- `REVIEW_REASONS == {gray_band, never_unsatisfiable, coherence}` and `REVIEW_STATUSES == {open, resolved_match, resolved_no_match, dismissed}` are single definitions imported by the accepted_values dbt test data
- Rollback is proven by an injected failure, not by inspection
- `bash scripts/gates.sh` green; INTERFACES entry lists the module's public symbols
