---
id: ER-051
title: "Generator part 2: corruptions.py, profiles.yaml, emit.py (base_10-identical headers), cli.py"
milestone: M2
status: in_progress
kind: code
size: M
gates: fast
depends_on: ["ER-039", "ER-041", "ER-050"]
spec_refs: ["s10-1", "s10-2", "s8-2", "s6", "s6-1", "s4-2", "s3"]
gap_refs: ["M14", "M21", "M24"]
provides: ["fixtures/generator/corruptions.py::CorruptionProfile", "fixtures/generator/corruptions.py::load_profiles", "fixtures/generator/corruptions.py::corrupt_record", "fixtures/generator/profiles.yaml", "fixtures/generator/emit.py::SOURCE_HEADERS", "fixtures/generator/emit.py::emit_corpus", "fixtures/generator/emit.py::CorpusSpec", "fixtures/generator/cli.py::main"]
consumes: ["fixtures/generator/personas.py::Persona", "fixtures/generator/personas.py::generate_personas", "src/er/config/loader.py::load_config", "src/er/config/schema.py::ErConfig", "src/er/std/address_parser.py::RegexV1Parser", "dbt/seeds/nickname_variants.csv", "fixtures/static/base_10/base/crm.csv", "fixtures/static/base_10/base/billing.csv", "fixtures/static/base_10/base/webforms.csv"]
owns: ["fixtures/generator/corruptions.py", "fixtures/generator/profiles.yaml", "fixtures/generator/emit.py", "fixtures/generator/cli.py", "tests/unit/generator/test_corruptions.py", "tests/unit/generator/test_emit.py"]
protected_paths: ["fixtures/static/base_10/", "fixtures/generator/personas.py", "configs/test.yaml", "dbt/seeds/nickname_variants.csv"]
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/generator/test_corruptions.py tests/unit/generator/test_emit.py -q"
branch: "ticket/ER-051-generator-part-2-corruptions-py-profiles"
commit: ""
spec_sha: "2abcfe433c322f74"
updated_at: "2026-08-15T07:03:52Z"
session: b0d01122-928d-494b-887e-440c8837d061
---
## Description

Completes the seeded synthetic generator of S10.1: per-source corruption profiles, the emitter that writes source CSVs, and a CLI entry point. S10.1 requires the generator to read nothing but `generator.seed` (S6) — no clock, no unseeded RNG — and to emit only address patterns the v1 parser handles. The emitted headers MUST be the literal `base_10` headers of S8.2 (themselves derived from `sources.<name>.columns`), because the same `stg_*` models and the same `er ingest` path consume both. Everything downstream in M3 depends on this: the committed fixture model (ER-056) is trained on this generator's 10k corpus, and the benchmark (M5) generates every scale from it.

## Scope

### In scope

- `corruptions.py`: typo injection, nickname substitution drawn only from the `nickname_variants` seed, per-field missingness, phone/date format drift, stale-address drift — each parameterised, each driven by a seeded RNG derived from `generator.seed` and the persona/source index
- `profiles.yaml`: one corruption profile per source (`crm`, `billing`, `webforms`) with literal rates, loaded and validated by `load_profiles`
- `emit.py`: `emit_corpus(spec, personas, out_dir)` writing `crm.csv`, `billing.csv`, `webforms.csv` plus a `truth.csv` ground-truth file, with `persona_id` as the last field of every source row (S8.2)
- `SOURCE_HEADERS`: the three literal header tuples, asserted equal to the committed `base_10` header rows
- Per-source `date_format` rendering taken from `configs/test.yaml` `sources.<name>.date_format` (`%m/%d/%Y` for billing, `%Y-%m-%d` for the other two)
- Configurable duplication factor: records per persona per source, so a `(personas, records)` pair is reproducible
- `cli.py`: `python -m fixtures.generator.cli --personas N --records M [--batch K] --seed S --out DIR`, exit 0, writing the four files

### Out of scope

- `personas.py` (ER-050 owns it — consume `generate_personas`)
- Reading `benchmarks/scales.yaml`: that file does not exist until M5. Scale shape arrives as explicit `--personas/--records/--batch` arguments
- Writing anything into `fixtures/static/` — `base_10` is hand-authored and is a protected path here
- Ingesting the generated corpus, or any lake/DuckDB access
- Training a model on the corpus (ER-056)

## Design decisions applied

Closes M14 (headers derived from `sources.<name>.columns`, never invented), M21 (`persona_id` truth column travels with every row so S8.5 metrics have a truth set) and M24 (the benchmark's corpus source). Two constraints are easy to miss: (1) S10.1 forbids any clock or unseeded RNG — derive every stream from `generator.seed` so two processes on different machines emit byte-identical files; (2) S4.2's `address_parse` is a regex/`usaddress` v1 parser and S10.1 says the generator emits only patterns it handles, so every emitted `address_line` MUST parse. Import every symbol named in `consumes` from INTERFACES.md; where a path there differs from the one written here, INTERFACES.md wins — do not re-implement.

## Acceptance criteria

- [ ] AC1: `emit.py::SOURCE_HEADERS['crm']` equals the literal first line of `fixtures/static/base_10/base/crm.csv` split on `,`, and the same holds for `billing` and `webforms`; the test reads the committed files rather than restating the headers
- [ ] AC2: Running `python -m fixtures.generator.cli --personas 40 --records 100 --seed 42 --out D` twice in two separate processes produces byte-identical `crm.csv`, `billing.csv`, `webforms.csv` and `truth.csv`; changing `--seed` to 43 changes at least one of them
- [ ] AC3: Every emitted source row carries `persona_id` as its last field; `truth.csv` lists exactly the emitted `(source_system, source_record_id, persona_id)` triples, and the number of distinct `persona_id` values equals `--personas` while total emitted rows equal `--records`
- [ ] AC4: For every emitted `address_line`, `RegexV1Parser().parse(line)` returns non-null `addr_number` and `addr_street` — zero unparseable addresses across a 1,000-record emission
- [ ] AC5: Every nickname substitution produced by `corrupt_record` appears as a pair in `dbt/seeds/nickname_variants.csv`; a substitution not present in the seed fails the test
- [ ] AC6: Phone format drift emits only the three S8.2 forms (`(415) 555-0132`, `415-555-0132`, `+14155550132` shapes) and every emitted variant of one persona's phone reduces to a single digit string after stripping non-digits
- [ ] AC7: Dates in `billing.csv` render as `%m/%d/%Y` and in `crm.csv`/`webforms.csv` as `%Y-%m-%d`, matching `configs/test.yaml` `sources.<name>.date_format`; a missing date is an empty field, never the literal `None`
- [ ] AC8: `load_profiles` rejects a `profiles.yaml` naming a source absent from `sources:` in the config, and rejects a rate outside `[0, 1]`

## Tests

- tests/unit/generator/test_emit.py::test_headers_match_base_10_literals
- tests/unit/generator/test_emit.py::test_emission_is_byte_reproducible_across_processes
- tests/unit/generator/test_emit.py::test_persona_id_is_last_column_and_truth_matches
- tests/unit/generator/test_emit.py::test_row_and_persona_counts_match_spec
- tests/unit/generator/test_emit.py::test_date_format_per_source
- tests/unit/generator/test_corruptions.py::test_nicknames_come_only_from_the_seed
- tests/unit/generator/test_corruptions.py::test_phone_drift_forms_reduce_to_one_number
- tests/unit/generator/test_corruptions.py::test_every_address_parses_under_regex_v1
- tests/unit/generator/test_corruptions.py::test_profile_validation_rejects_unknown_source_and_bad_rate

## Verification

```bash
uv run pytest tests/unit/generator/test_corruptions.py tests/unit/generator/test_emit.py -q
uv run ruff check fixtures/generator && uv run ruff format --check fixtures/generator
uv run python -m fixtures.generator.cli --personas 40 --records 100 --seed 42 --out /tmp/er-gen-a && uv run python -m fixtures.generator.cli --personas 40 --records 100 --seed 42 --out /tmp/er-gen-b && diff -r /tmp/er-gen-a /tmp/er-gen-b
```

## Definition of Done

- Acceptance criteria met and the verify command passes
- `fixtures/generator/` contains no `datetime.now`, `time.time` or module-level `random.` call (grep-asserted in the test)
- `emit_corpus` writes under the caller-supplied `--out` only; nothing writes into `fixtures/static/`
- `SOURCE_HEADERS` is the single definition of the three header tuples; no second copy in the repo
- `ruff check` and `ruff format --check` clean on `fixtures/generator`
- `provides` entries importable as written and recorded in INTERFACES.md
- Committed on a branch off main
