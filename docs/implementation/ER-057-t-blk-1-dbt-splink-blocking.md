---
id: ER-057
title: "T-BLK-1: dbt/Splink blocking parity on base_10 + tests/helpers/pairs.py"
milestone: M3
status: todo
kind: code
size: M
gates: full
depends_on: ["ER-047", "ER-049", "ER-056"]
spec_refs: ["s4-2", "s8-3", "s5-0", "s4-3-4", "s8-2"]
gap_refs: ["M12"]
provides: ["tests/helpers/pairs.py::canonical_pairs_from_blocking_keys", "tests/helpers/pairs.py::splink_blocked_pairs", "tests/helpers/pairs.py::symmetric_difference_report", "tests/integration/test_blocking_parity.py::test_dbt_and_splink_pair_sets_match"]
consumes: ["src/er/matching/model.py::blocking_rules_from_config", "src/er/matching/model.py::build_settings", "src/er/matching/splink_env.py::splink_api", "src/er/matching/splink_env.py::assert_no_splink_relations_in_lake", "src/er/entities/ids.py::canonicalize_pair", "tests/helpers/model.py::load_fixture_model", "tests/conftest.py::lake_ns", "fixtures/static/base_10/base/crm.csv"]
owns: ["tests/helpers/pairs.py", "tests/integration/test_blocking_parity.py", "tests/unit/matching/test_pairs_helper.py"]
protected_paths: ["dbt/models/intermediate/int_blocking_keys.sql", "dbt/macros/blocking/", "src/er/matching/model.py", "fixtures/static/base_10/"]
extra_paths: []
attempts: 0
verify: "bash scripts/ci/itest.sh tests/integration/test_blocking_parity.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Ships T-BLK-1, the parity check S4.2 calls "the only thing converting 'mirrored' from aspiration into a checked invariant": on `base_10`, the DISTINCT canonicalised pair set derived from `int_blocking_keys` equals Splink's blocked pair set exactly. M12 is that the same blocking logic has three descriptions with no generation direction, so drift between the dbt key table and Splink's `block_on` rules is silent and shows up later as a T-INC-1 failure attributed to clustering. The ticket also extracts the two pair-set derivations into `tests/helpers/pairs.py` so later oracles (T-INC-3, T-INV-1, the metrics tickets) share one implementation.

## Scope

### In scope

- `canonical_pairs_from_blocking_keys(conn)` — self-join `int_blocking_keys` on `(key_type, key_value)`, drop self-pairs, canonicalise via `canonicalize_pair`, `SELECT DISTINCT`
- `splink_blocked_pairs(conn, cfg, settings)` — the blocked pair set Splink generates from `blocking_rules_from_config(cfg)[1]`, canonicalised and deduplicated, produced through `splink_api()` so intermediates stay in `splink_scratch`
- `symmetric_difference_report(a, b)` — the failure message, listing missing/extra pairs with the `key_type` that would have produced each
- The T-BLK-1 integration test on `base_10`, plus the NULL/empty-key and multiplicity-collapse assertions of S4.2
- A unit test of the helpers over a hand-built key table

### Out of scope

- Editing `int_blocking_keys.sql`, the blocking macro or `blocking_rules_from_config` — all protected; a parity failure is a defect in ER-046/ER-047 and is recorded as a blocker there
- Scoring, `match_scores`, thresholds (ER-058)
- The incremental two-pass candidate generation (ER-065)
- Blocking recall as a quality metric (ER-067 owns `pairwise_metrics`)

## Design decisions applied

Closes M12. Constraints: (1) both sides MUST originate from a single `blocking_rules_from_config(cfg)` call — the dbt var payload and the `BlockingRuleCreator` list are the two elements of its return tuple, and the test asserts their `expr` strings are the same strings, which is what makes the parity meaningful; (2) the dbt-derived set MUST be `SELECT DISTINCT` over canonicalised pairs, because Splink deduplicates across rules via preceding-rule exclusion and a key-table self-join does not (S4.2); (3) NULL and empty key values are never emitted and never block, on either side. Path note: S8.3 lists T-BLK-1 at `tests/integration/test_blocking.py`; the board pins `tests/integration/test_blocking_parity.py`. Ship it once, at the board path, keeping the S8.3 node name `test_dbt_and_splink_pair_sets_match` — do not create a second copy — and record the path deviation for ER-103's S8.3 node-id lint. Import consumed symbols from INTERFACES.md; where a path differs, INTERFACES.md wins.

## Acceptance criteria

- [ ] AC1: On `base_10` after standardization, `canonical_pairs_from_blocking_keys(conn) == splink_blocked_pairs(conn, cfg, settings)` as sets, asserted in both directions; on an injected divergence (one `key_type` dropped from the dbt payload) the test fails and prints the symmetric difference with per-pair `key_type` attribution
- [ ] AC2: Both pair sets are built from one `blocking_rules_from_config(cfg)` return value: the set of `expr` strings in the dbt var payload equals the set of `expr` strings the `BlockingRuleCreator` list was constructed from
- [ ] AC3: Every pair returned by either helper satisfies `rec_a_key < rec_b_key`, and neither set contains a self-pair
- [ ] AC4: `select count(*) from lake.main.int_blocking_keys where key_value is null or key_value = ''` is 0, and the two records whose email is nulled by `email_norm` appear in no `email_exact` pair on either side
- [ ] AC5: At least one `base_10` pair is produced by two different `key_type`s, and `canonical_pairs_from_blocking_keys` returns it exactly once while the underlying self-join returns it more than once — the DISTINCT requirement is exercised, not assumed
- [ ] AC6: `assert_no_splink_relations_in_lake(conn)` passes after `splink_blocked_pairs` runs
- [ ] AC7: The unit test over a hand-built key table returns the expected pair set for a table containing a NULL key, an empty-string key, a duplicated key row and a single-record key group

## Tests

- tests/integration/test_blocking_parity.py::test_dbt_and_splink_pair_sets_match
- tests/integration/test_blocking_parity.py::test_both_sides_come_from_one_generator_call
- tests/integration/test_blocking_parity.py::test_pairs_are_canonical_and_self_free
- tests/integration/test_blocking_parity.py::test_null_and_empty_keys_never_block
- tests/integration/test_blocking_parity.py::test_multiplicity_collapses_to_one_pair
- tests/integration/test_blocking_parity.py::test_no_splink_relations_in_lake
- tests/unit/matching/test_pairs_helper.py::test_pair_derivation_over_hand_built_key_table

## Verification

```bash
bash scripts/ci/itest.sh tests/integration/test_blocking_parity.py -q
uv run pytest tests/unit/matching/test_pairs_helper.py -q
uv run mypy --strict tests/helpers
```

## Definition of Done

- Acceptance criteria met and the verify command passes
- `tests/helpers/pairs.py` is the only place either pair set is derived; no later ticket needs its own copy
- Failure output prints the symmetric difference, not just a count
- Protected paths unmodified; any parity failure filed as a blocker against the owning ticket
- `mypy --strict tests/helpers` clean
- `provides` entries recorded in INTERFACES.md
- Committed on a branch off main
