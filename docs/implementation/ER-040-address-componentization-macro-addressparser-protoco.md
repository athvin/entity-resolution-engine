---
id: ER-040
title: "Address componentization macro + AddressParser Protocol + RegexV1Parser Python oracle + address_parser_version"
milestone: M2
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-037"]
spec_refs: ["s3", "s4-2", "s5", "s6", "s6-1", "s8-2", "s8-4"]
gap_refs: ["M14", "MINOR-address-parser"]
provides: ["dbt/macros/std/address_parse.sql::address_parse", "src/er/std/address.py::AddressParser", "src/er/std/address.py::AddressComponents", "src/er/std/address.py::RegexV1Parser", "src/er/std/address.py::get_address_parser", "tests/unit/std/address_cases.py::ADDRESS_CASES"]
consumes: ["tests/unit/dbt/harness.py::MacroHarness", "tests/unit/dbt/harness.py::eval_macro", "dbt/macros/std/lowercase_trim.sql::lowercase_trim", "dbt/macros/std/null_semantics.sql::null_semantics", "src/er/config/schema.py::Config", "src/er/versions.py", "src/er/errors.py"]
owns: ["dbt/macros/std/address_parse.sql", "src/er/std/__init__.py", "src/er/std/address.py", "tests/unit/std/__init__.py", "tests/unit/std/address_cases.py", "tests/unit/std/test_address_parser.py", "tests/unit/dbt/test_address.py"]
protected_paths: []
extra_paths: ["src/er/versions.py", "tests/unit/test_package_layout.py"]
attempts: 0
verify: "uv run pytest tests/unit/dbt/test_address.py tests/unit/std/test_address_parser.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Ship the S4.2 `address_parse` macro emitting the six `addr_*` columns S5 declares, together with the `AddressParser` Protocol and its v1 implementation `RegexV1Parser` as a Python oracle. The macro componentizes the single `address_line` source column (S6) into `addr_number`, `addr_street` and `addr_unit`; `addr_city`, `addr_region` and `addr_postal` are normalized pass-throughs of their own source columns. `versions.address_parser_version` gets a real role: it selects the parser and is asserted at CLI start, so a parser change is a config change that moves `config_hash` rather than a silent corpus rewrite.

## Scope

### In scope

- `AddressComponents` (frozen dataclass: `addr_number`, `addr_street`, `addr_unit`) and the `AddressParser` Protocol with `parse(address_line: str | None) -> AddressComponents`.
- `RegexV1Parser` implementing the v1 grammar with `version = '1'`, plus `get_address_parser(cfg)` selecting by `versions.address_parser_version` and raising the config error (exit `2`) for an unknown version.
- `address_parse(address_line_col, city_col, region_col, postal_col)` emitting exactly the six aliases `addr_number, addr_street, addr_unit, addr_city, addr_region, addr_postal`.
- A single committed case table shared by both test files, so the SQL macro and the Python oracle are compared against the same inputs.
- Normalized pass-through of city/region/postal via `lowercase_trim` + `null_semantics`.

### Out of scope

- `usaddress`, `libpostal` or any other parsing dependency: adding one requires an S2.1 pin row and is a spec amendment.
- ZIP+4 collapsing or any postal reformatting beyond `lowercase_trim`/sentinel handling — unspecified in v1; fixtures must not contain forms the v1 grammar does not handle (S8.2).
- A second address column: each source carries exactly one `address_line` (S8.2).
- The `base_10` address rows themselves (ER-041) and the generator's address emission (ER-051).
- Editing `DesignDoc.md`.

## Design decisions applied

Implements gap entries M14 and MINOR-address-parser. Constraints: (1) **Module home.** S3's tree names no location for the parser; it goes in `src/er/std/address.py` because the board pins the test node id `tests/unit/std/test_address_parser.py`. If `tests/unit/test_package_layout.py` enumerates the S3 tree exactly, add the one directory to its allowlist — do not move the module and do not edit `DesignDoc.md`; a layout amendment is a separate spec ticket. (2) **The macro and the oracle must agree.** They are two implementations of one grammar, which is exactly the drift shape T-BLK-1 exists to catch elsewhere; the parity test over the shared case table is the primary acceptance criterion and is stronger than either side's own assertions. (3) Only `addr_number`, `addr_street` and `addr_unit` are parsed — S6 states `addr_city`, `addr_region` and `addr_postal` map directly from their own source columns. (4) `address_parser_version` is a `versions:` field (V13) and therefore inside `config_hash`, so a parser swap is caught by the S4.0 drift guard; the runtime assertion that the selected parser's version equals the configured value is what makes that meaningful. (5) The generator emits only patterns the v1 parser handles (S4.2), so an unparsable address yields NULL components rather than a guess — never a partial street with the number folded in.

## Acceptance criteria

- [ ] AC1: For every entry in the committed case table, `eval_macro('address_parse', case.address_line)` equals `RegexV1Parser().parse(case.address_line)` field for field — the SQL macro and the Python oracle never disagree.
- [ ] AC2: The case table covers at least 20 inputs including `'123 Main St'`, `'123 Main St Apt 4B'`, `'123 N Main Street Unit 12'`, `'123  Main   St'`, `'PO Box 7'`, a unit-only line, a street-only line with no number, an empty string, a `null_semantics` sentinel and NULL; each has its expected three components committed alongside it.
- [ ] AC3: `'123 Main St Apt 4B'` yields `addr_number='123'`, `addr_street='main st'`, `addr_unit='apt 4b'`; `'123 Main St'` yields `addr_unit` NULL; an unparsable line yields all three components NULL rather than a partial parse.
- [ ] AC4: `address_parse` expands to exactly six projections with the S5 alias names, and `addr_city`/`addr_region`/`addr_postal` are the `lowercase_trim`ped pass-throughs of their own source columns (an `address_line` change never alters them).
- [ ] AC5: `get_address_parser(cfg)` returns a `RegexV1Parser` whose `version` equals `cfg.versions.address_parser_version` for `'1'`, and raises the config error whose exit code is `2` for `'2'`.
- [ ] AC6: `RegexV1Parser` satisfies the `AddressParser` Protocol under `mypy --strict`, and `parse` is idempotent in the sense that re-parsing a rendered `'<addr_number> <addr_street> <addr_unit>'` line reproduces the same components.
- [ ] AC7: `tests/unit/std/test_address_parser.py` and `tests/unit/dbt/test_address.py` both run on a bare runner with no dbt subprocess, no lake and no new third-party dependency.

## Tests

- tests/unit/dbt/test_address.py::test_macro_matches_the_python_oracle_on_every_case
- tests/unit/dbt/test_address.py::test_emits_six_addr_aliases
- tests/unit/dbt/test_address.py::test_city_region_postal_are_normalized_passthroughs
- tests/unit/std/test_address_parser.py::test_regex_v1_parses_the_committed_cases
- tests/unit/std/test_address_parser.py::test_unparsable_line_yields_all_null_components
- tests/unit/std/test_address_parser.py::test_get_address_parser_selects_by_version_and_rejects_unknown
- tests/unit/std/test_address_parser.py::test_parse_is_idempotent_on_a_rendered_line

## Verification

```bash
uv run pytest tests/unit/dbt/test_address.py tests/unit/std/test_address_parser.py -q
uv run mypy --strict src/er
uv run dbt parse --project-dir dbt --profiles-dir dbt/profiles --target mem
uv run pytest tests/unit/test_package_layout.py -q
uv run ruff check . && uv run ruff format --check .
```

## Definition of Done

- All acceptance criteria have a named passing test
- Verify command passes on a bare runner
- The SQL macro and the Python oracle are compared against one shared committed case table and agree on every entry
- `address_parser_version` selects the parser and an unknown version exits `2`
- No new third-party dependency and no S2.1 row required
- `tests/unit/test_package_layout.py` and `dbt parse --target mem` still pass
- mypy --strict and ruff clean
