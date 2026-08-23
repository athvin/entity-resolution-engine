---
id: ER-087
title: "Survivorship macros: five literal ORDER BY fragments + mandatory terminal record_key ASC + tiebreak_deterministic lineage"
milestone: M4
status: done
kind: code
size: M
gates: fast
depends_on: ["ER-012", "ER-038", "ER-043"]
spec_refs: ["s4-6", "s5", "s5-0", "s6", "s6-1"]
gap_refs: ["M11", "B4"]
provides: ["dbt/macros/survivorship/survivorship.sql::survivorship_order_by", "dbt/macros/survivorship/survivorship.sql::survivorship_decision", "dbt/macros/survivorship/rules.sql::rule_source_priority", "dbt/macros/survivorship/rules.sql::rule_recency", "dbt/macros/survivorship/rules.sql::rule_frequency", "dbt/macros/survivorship/rules.sql::rule_completeness", "dbt/macros/survivorship/rules.sql::rule_validated", "dbt/macros/survivorship/rules.sql::TERMINAL_TIEBREAK"]
consumes: ["tests/unit/dbt/harness.py::render_macro", "src/er/lake/columns.py::GOLDEN_SURVIVABLE_COLUMNS", "src/er/config/schema.py::SurvivorshipConfig", "relation:int_std_records"]
owns: ["dbt/macros/survivorship/", "tests/unit/dbt/test_survivorship_macros.py"]
protected_paths: []
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/dbt/test_survivorship_macros.py -q"
branch: "ticket/ER-087-survivorship-macros-five-literal-order-by"
commit: "3253477cb89fc32f759adff41c2bc3ef79fb0d99"
spec_sha: "7467bdacba1bd84c"
updated_at: "2026-08-23T14:34:54Z"
session: c379438e-8c95-4be8-8f5f-3832d8ceafaa
---
## Description

M11 found five survivorship rules named and none defined, with chains that are not total orders — so the golden winner depends on physical row order, which differs between the touched-subset and full-corpus materialisations. S4.6 fixes each rule as one literal `ORDER BY` fragment with `record_key ASC` as a mandatory terminal element. This ticket ships those five macros, the chain concatenator, and the rule-attribution expression that lets `golden_lineage.rule` name the deciding rule or `tiebreak_deterministic`. It is the pure layer under ER-088's marts and is tested with no lake.

## Scope

### In scope

- One macro per rule emitting exactly the S4.6 literal `ORDER BY` fragment
- `survivorship_order_by(attribute, chain)` concatenating fragments in chain order and always terminating with `record_key ASC`, never duplicating it
- `survivorship_decision(attribute, chain)` emitting both the rank-1 winner and the deciding-rule name, evaluated by comparing the rank-1 and rank-2 rows' per-rule sort keys
- Unit tests against the ER-037 macro harness (Jinja render + in-process DuckDB) with a designed tie per rule and a row-shuffle determinism arm

### Out of scope

- `golden_records.sql` / `golden_lineage.sql` and anything touching the lake (ER-088)
- Config validation of the chains — V2–V5 are ER-012's; this ticket assumes a validated chain
- The `base_10` expected lineage values and rule-coverage table (ER-090)
- `golden_display` presentation transforms (ER-089)

## Design decisions applied

Implements gaps M11 and B4's survivorship half. Constraints easy to miss: (1) `record_key ASC` is a MANDATORY terminal element of EVERY chain — it is what makes each chain a total order and it is load-bearing for T-INC-1 and T-GOLD-1, because without it the winner depends on physical row order; the config normalization in S6.1 already appends it, so the macro must be idempotent about it rather than appending a second copy; (2) the fragments are literal — `recency` is `COALESCE(updated_at_source, ingested_at) DESC` (never bare `updated_at_source`), `completeness` is `(value IS NOT NULL) DESC, length(value) DESC`, `validated` is `<attr>_valid DESC NULLS LAST`, `frequency` is `count(*) OVER (PARTITION BY entity_id, value) DESC`, `source_priority` is `sources[source_system].priority_rank ASC`; (3) deciding-rule attribution is pinned here: the deciding rule is the FIRST rule in the chain whose sort key differs between the rank-1 and rank-2 rows; when there is no rank-2 row, or every rule's key ties, the rule is `tiebreak_deterministic` — that vocabulary is closed by S5's `golden_lineage.rule` comment; (4) `validated` has an input for both `email` and `phone_e164` because `int_std_records` carries `phone_valid` as well as `email_valid` (S4.6) — do not special-case email.

## Acceptance criteria

- [ ] AC1: Each of the five rule macros renders to exactly the literal `ORDER BY` fragment in the S4.6 table, compared as whitespace-normalised string equality
- [ ] AC2: `survivorship_order_by('email', ['validated','source_priority','recency'])` renders the three fragments comma-joined in chain order followed by `record_key ASC`, and rendering a chain that already ends in `record_key ASC` produces exactly one occurrence of it
- [ ] AC3: For each of the five rules there is a two-row input in which only that rule's key differs: the generated window selects the expected row and `survivorship_decision` reports that rule's name
- [ ] AC4: A two-row input tying on every rule in the chain selects the lexically smaller `record_key` and reports `tiebreak_deterministic`; a single-candidate input also reports `tiebreak_deterministic`
- [ ] AC5: A row with NULL `updated_at_source` and a later `ingested_at` beats a row with an earlier `updated_at_source` under `recency`
- [ ] AC6: Shuffling the physical order of the input rows changes neither the winner nor the reported rule for every case above
- [ ] AC7: `survivorship_order_by` with an unknown rule name raises a dbt compilation error whose message names the offending rule
- [ ] AC8: `uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem` exits 0 with the macros present

## Tests

- tests/unit/dbt/test_survivorship_macros.py::test_each_rule_fragment_is_literal
- tests/unit/dbt/test_survivorship_macros.py::test_chain_terminates_in_record_key_asc_exactly_once
- tests/unit/dbt/test_survivorship_macros.py::test_source_priority_decides_designed_tie
- tests/unit/dbt/test_survivorship_macros.py::test_recency_coalesces_ingested_at
- tests/unit/dbt/test_survivorship_macros.py::test_frequency_picks_modal_value
- tests/unit/dbt/test_survivorship_macros.py::test_completeness_prefers_non_null_then_longer
- tests/unit/dbt/test_survivorship_macros.py::test_validated_orders_valid_first_nulls_last
- tests/unit/dbt/test_survivorship_macros.py::test_full_tie_reports_tiebreak_deterministic
- tests/unit/dbt/test_survivorship_macros.py::test_winner_invariant_under_row_shuffle
- tests/unit/dbt/test_survivorship_macros.py::test_unknown_rule_raises

## Verification

```bash
uv run pytest tests/unit/dbt/test_survivorship_macros.py -q
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run ruff check .
uv run ruff format --check .
```

## Definition of Done

- Five rule macros, one chain concatenator and one decision macro, all under `dbt/macros/survivorship/`
- Every fragment matches S4.6 literally; a diff against that table is a test failure, not a review comment
- The deciding-rule attribution rule (first differing key, else `tiebreak_deterministic`) is documented in a macro comment as the contract ER-088 and ER-090 rely on
- Row-shuffle invariance is asserted for every rule, proving each chain is a total order
- `uv run pytest tests/unit/dbt/test_survivorship_macros.py -q` passes and failed before the change
