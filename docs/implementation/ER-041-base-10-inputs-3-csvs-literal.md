---
id: ER-041
title: "base_10 inputs: 3 CSVs + literal headers into S8.2, truth.csv, machine-checked trap index, designed rule-coverage + gray-band + recency guards, sources.columns in configs/test.yaml"
milestone: M2
status: todo
kind: fixture
size: L
gates: fast
depends_on: ["ER-028", "ER-029", "ER-038", "ER-039", "ER-040"]
spec_refs: ["s8-2", "s8-2-1", "s6", "s6-1", "s4-2", "s4-6", "s4-3", "s8-5"]
gap_refs: ["M14", "M21", "M11", "B4"]
provides: ["fixtures/static/base_10/base/crm.csv", "fixtures/static/base_10/base/billing.csv", "fixtures/static/base_10/base/webforms.csv", "fixtures/static/base_10/truth.csv", "fixtures/static/base_10/traps.csv", "tests/unit/fixtures/test_base_10.py", "configs/test.yaml::sources.columns"]
consumes: ["tests/helpers/expected.py::load_scenario", "scripts/validate_fixtures.py", "src/er/ingest/hashing.py::content_hash", "src/er/config/schema.py::Config", "src/er/std/address_parser.py::RegexV1Parser", "dbt/seeds/nickname_variants.csv"]
owns: ["fixtures/static/base_10/base/crm.csv", "fixtures/static/base_10/base/billing.csv", "fixtures/static/base_10/base/webforms.csv", "fixtures/static/base_10/truth.csv", "fixtures/static/base_10/traps.csv", "tests/unit/fixtures/test_base_10.py"]
protected_paths: []
extra_paths: ["configs/test.yaml", "configs/default.yaml"]
attempts: 0
verify: "uv run pytest tests/unit/fixtures/test_base_10.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Author the hand-written `base_10` corpus: three per-source input CSVs under `fixtures/static/base_10/base/`, a `truth.csv` carrying the persona label per row, and a `traps.csv` index naming each S8.2 designed trap and the rows that construct it. The CSV headers are derived from `sources.<name>.columns` in `configs/test.yaml` (S6) and are written back into S8.2 as the literal header block, so a header edit without a config edit fails V11. Every downstream M2–M4 scenario test reads this fixture, so the ticket also ships the machine checks that keep it from drifting: truth counts (23 records / 10 personas / 18 true pairs, S8.2), the survivorship rule-coverage and recency guards (S4.6), and the cross-persona gray-band construction (S8.2).

## Scope

### In scope

- Three CSVs under `fixtures/static/base_10/base/` with the S8.2 literal headers, `persona_id` last on every row
- `truth.csv` (persona → record) with persona record counts 4,3,3,2,2,2,2,2,2,1
- `traps.csv`: one row per S8.2 trap naming the trap and the participating `(source_system, source_record_id)` values
- `sources.columns`, `record_id_column`, `updated_at_column`, `date_format` for crm/billing/webforms in `configs/test.yaml` (and the same shape in `configs/default.yaml`)
- Pasting the three derived header lines into S8.2's header block if they are not already literal there
- `tests/unit/fixtures/test_base_10.py`: header↔config derivation, truth counts, trap index completeness, gray-band uniqueness, rule-coverage, recency and 2-record-persona guards

### Out of scope

- `expected/` files of any kind — `std_hashes.csv` / `membership.csv` / `events.csv` are ER-045 and `golden.csv` / `lineage.csv` are ER-090
- Any dbt model, macro or `int_std_records` row (nothing here runs the pipeline)
- Trap outcomes that are only decidable under a trained model (exactly-one-gray-band-pair *by score*, missed-edge tolerance) — those are ER-060's post-match gate
- Other scenarios (`incremental_batch`, `merge_scenario`, `deletion_scenario`, …)

## Design decisions applied

Closes M14 (source column mapping + literal headers), M21 (machine-checked ground truth), M11 (every survivorship rule has a deciding case) and B4 (the golden fixture needs a corpus that exercises the survivable column set). Constraints that are easy to miss: the gray-band pair MUST be cross-persona (S8.2 states why — a same-persona pair makes entity count 11 and fails T-MATCH-1b with no pointer to the cause); every true pair of a 2-record persona must be carried by an exact email or exact phone key, because a below-`auto_merge` true pair is only tolerable inside the 4/3/3 personas; `test@test.com` is nulled by `email_norm` from `standardization.email_placeholders`, never by `null_semantics`; each source has exactly ONE address column (`address_line`), with any unit detail authored inline; crm/webforms use `%Y-%m-%d` and billing uses `%m/%d/%Y`; `persona_id` is truth-only and is stripped by the loader before `er ingest`.

## Acceptance criteria

- [ ] AC1: The first line of each of the three CSVs equals the header the test derives from `configs/test.yaml` — `record_id_column`, then the `columns` values in canonical-attribute order, then `updated_at_column`, then `persona_id` — and equals the corresponding literal block in S8.2 (test reads both, hard-codes neither).
- [ ] AC2: `truth.csv` and the three CSVs agree on 23 records over 10 personas, the sorted persona sizes are `[4,3,3,2,2,2,2,2,2,1]`, and the derived true-pair count is exactly 18 over a `C(23,2)=253` universe.
- [ ] AC3: `traps.csv` names all eight S8.2 traps and for each one the test resolves the cited rows and asserts the construction: the nickname pair is crm "Robert Chen" / webforms "Bob Chen" whose phone fields both normalize to one E.164 value; the household pair shares an identical `address_line` with different family names, different DOBs and disjoint email and phone; exactly two rows of two different personas carry `test@test.com`; exactly four rows have an empty email field; one persona's three rows carry `(415) 555-0132`, `415-555-0132` and `+14155550132`.
- [ ] AC4: Exactly one cross-persona record pair in the corpus shares `addr_postal` and the first four characters of `family_name` while differing on `birth_date` and sharing neither email nor phone, and the test asserts its two records belong to different personas.
- [ ] AC5: For each of `source_priority`, `recency`, `frequency`, `completeness` and `validated` there is at least one `(persona, attribute)` in which that rule is the first element of the configured chain able to separate the persona's contributing rows, and exactly one persona holds two records from the *same* source with equal `priority_rank`, equal `updated_at`, and non-null equal-length but different `given_name` values (the `tiebreak_deterministic` row).
- [ ] AC6: Every persona/attribute where `recency` decides has a non-null, pairwise-distinct `updated_at` value on each contributing row, so `COALESCE(updated_at_source, ingested_at)` never falls back to the volatile `ingested_at`.
- [ ] AC7: Every true pair inside a 2-record persona shares an exact normalized email or an exact normalized phone (checked through the ER-038/ER-040 Python oracles), and no 2-record persona is connected only by a name/postal similarity.
- [ ] AC8: Every `address_line` parses through `RegexV1Parser` into all six `addr_*` components with no leftover text, and no `record_id_column` value contains `':'`.

## Tests

- tests/unit/fixtures/test_base_10.py::test_headers_are_derived_from_config
- tests/unit/fixtures/test_base_10.py::test_truth_counts_23_records_10_personas_18_pairs
- tests/unit/fixtures/test_base_10.py::test_trap_index_is_complete_and_constructions_hold
- tests/unit/fixtures/test_base_10.py::test_gray_band_candidate_is_unique_and_cross_persona
- tests/unit/fixtures/test_base_10.py::test_survivorship_rule_coverage_and_tiebreak_row
- tests/unit/fixtures/test_base_10.py::test_recency_never_decided_by_ingested_at
- tests/unit/fixtures/test_base_10.py::test_two_record_personas_have_an_exact_key
- tests/unit/fixtures/test_base_10.py::test_addresses_parse_and_ids_have_no_colon

## Verification

```bash
uv run pytest tests/unit/fixtures/test_base_10.py -q
uv run python scripts/validate_fixtures.py fixtures/static/base_10
uv run pytest tests/unit/test_config_schema.py tests/unit/test_config_validators.py -q
python3 scripts/lint_spec.py --part a DesignDoc.md && python3 scripts/lint_spec.py --part b DesignDoc.md
```

## Definition of Done

- All acceptance criteria met and the verify command passes
- `configs/test.yaml` and `configs/default.yaml` validate (V11 passes: every canonical attribute mapped, `priority_rank` unique and positive)
- S8.2's literal header block equals the committed CSV headers; `lint_spec.py` still green on both parts
- `fixtures/static/base_10/` contains only `base/`, `truth.csv` and `traps.csv` — no `expected/` directory is created here
- `persona_id` is present on every input row and is documented as truth-only
- Committed on main with the board updated
