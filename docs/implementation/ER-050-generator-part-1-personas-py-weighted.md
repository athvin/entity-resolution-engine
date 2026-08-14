---
id: ER-050
title: "Generator part 1: personas.py, weighted-frequency name lists, household rate, seeded RNG"
milestone: M2
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-011", "ER-040"]
spec_refs: ["s10-1", "s10-2", "s6", "s4-2", "s8-2", "s12"]
gap_refs: ["M26", "M8", "M24"]
provides: ["fixtures/generator/__init__.py", "fixtures/generator/personas.py::Persona", "fixtures/generator/personas.py::generate_personas", "fixtures/generator/data/given_names.csv", "fixtures/generator/data/family_names.csv", "tests/unit/generator/test_personas.py"]
consumes: ["src/er/config/schema.py::Config", "configs/test.yaml", "src/er/std/address_parser.py::RegexV1Parser", "src/er/std/address_parser.py::AddressParser"]
owns: ["fixtures/generator/__init__.py", "fixtures/generator/personas.py", "fixtures/generator/data/given_names.csv", "fixtures/generator/data/family_names.csv", "tests/unit/generator/test_personas.py"]
protected_paths: ["src/er/std/address_parser.py"]
extra_paths: []
attempts: 0
verify: "uv run pytest tests/unit/generator/test_personas.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

First half of the seeded synthetic generator (S10.1): `personas.py` produces ground-truth persons — names drawn from committed weighted frequency lists so term-frequency adjustments see realistic skew, plus emails, phones, addresses and DOBs — from `generator.seed` in the tenant config and a requested persona count. A configurable household rate makes some personas share an address without sharing any identifying attribute, reproducing `base_10`'s shared-household trap at scale. Nothing reads a clock or an unseeded RNG, because the 10k corpus this produces is the input T-TRAIN-1's byte-for-byte model reproduction and the S10.4 benchmark fingerprint both depend on.

## Scope

### In scope

- `Persona` (frozen dataclass): `persona_id`, given/family name, email, phone, the six `addr_*` components with the single-line `address_line`, `birth_date`, household id
- `generate_personas(seed: int, n: int, household_rate: float) -> list[Persona]`, fully determined by its arguments
- Committed weighted frequency lists for given and family names (long-tailed, so TF adjustments have skew to see)
- Household construction: members share all six address components and share no email, phone, name or DOB
- Address generation restricted to patterns `RegexV1Parser` handles
- Unit tests: cross-process determinism, seed sensitivity, no clock / no global RNG, skew, household properties, address round-trip

### Out of scope

- `corruptions.py`, `emit.py`, per-source CSV rendering and the generator CLI (ER-051)
- `benchmarks/scales.yaml` and scale definitions (M5, ER-096) — `n` and the household rate are arguments here
- Writing anything to `storage.drop_dir` or to the lake
- Training, `model_test_v1.json`, or any Splink call
- Editing `RegexV1Parser`; the generator adapts to the parser, never the reverse

## Design decisions applied

Serves M26 (`generator.seed` is a config field, not a literal), M8 (a byte-reproducible corpus is the precondition for T-TRAIN-1's byte-equal model) and M24 (the benchmark fingerprint records the seed and every run must be reproducible from it); S12 moves the generator into M2 because the deletion, supersession and correction scenarios need seeded corpora larger than a human should hand-write. Easy to miss: no `time`/`datetime.now`, no module-level `random.*` call and no dependence on `set`/`dict` iteration order — the only entropy source is a `Random` instance constructed from the seed, and determinism must hold across processes, not just within one; household members must not share email, phone or DOB or the ground truth is contaminated and the "MUST NOT merge" guarantee becomes untestable; emails and phones are unique across non-household personas so no cross-persona link is designed in by accident; only address patterns the v1 parser handles may be emitted (S10.1).

## Acceptance criteria

- [ ] AC1: `generate_personas(seed=42, n=4000, household_rate=0.1)` returns 4000 personas, and the SHA-256 of their canonical JSON serialisation is identical when computed in a freshly spawned subprocess — determinism holds across processes, not merely within one.
- [ ] AC2: Calling `random.seed(0)` and shuffling global RNG state between two generator calls leaves the output identical, and the module contains no import of `time`/`datetime.now` and no module-level `random.*` call (asserted by AST inspection).
- [ ] AC3: Changing only the seed changes at least 99% of the generated personas; changing only `n` from 4000 to 4001 does not change the total count of a second independent draw at 4000.
- [ ] AC4: The surname distribution over 4000 personas is long-tailed rather than uniform: the most frequent family name covers at least 2% of personas, the top ten cover at least 15%, and the empirical relative frequencies track the committed weight list within a stated tolerance.
- [ ] AC5: At `household_rate=0.1`, the fraction of personas sharing an address with at least one other persona is 0.10 ± 0.02; every household's members share all six `addr_*` components exactly and share no `email`, `phone`, `family_name` or `birth_date`.
- [ ] AC6: `persona_id` values are unique and zero-padded to a fixed width, and across all 4000 personas every `email` and every `phone` is unique.
- [ ] AC7: Every generated `address_line` parses through `RegexV1Parser` into components equal to the persona's six `addr_*` values, for 100% of a 4000-persona draw.
- [ ] AC8: Every `birth_date` is a full-precision date renderable under both `%Y-%m-%d` and `%m/%d/%Y`, and no persona carries a year-only DOB.

## Tests

- tests/unit/generator/test_personas.py::test_output_is_byte_identical_across_processes
- tests/unit/generator/test_personas.py::test_no_clock_and_no_global_rng
- tests/unit/generator/test_personas.py::test_seed_sensitivity
- tests/unit/generator/test_personas.py::test_name_frequency_skew_matches_weight_lists
- tests/unit/generator/test_personas.py::test_household_rate_and_disjoint_identifiers
- tests/unit/generator/test_personas.py::test_persona_ids_emails_and_phones_are_unique
- tests/unit/generator/test_personas.py::test_addresses_round_trip_through_regex_v1_parser
- tests/unit/generator/test_personas.py::test_birth_dates_are_full_precision

## Verification

```bash
uv run pytest tests/unit/generator/test_personas.py -q
uv run mypy --strict fixtures/generator
uv run ruff check fixtures/generator
bash scripts/gates.sh
```

## Definition of Done

- All acceptance criteria met and the verify command passes
- `generate_personas` is a pure function of `(seed, n, household_rate)` with no I/O beyond reading the committed weight lists
- Weighted name lists are committed under `fixtures/generator/data/` with their provenance noted
- Cross-process determinism is proven by a subprocess-based test, not by an in-process re-call
- `RegexV1Parser` unmodified
- Committed on main with the board updated
