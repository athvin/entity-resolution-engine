---
id: ER-004
title: "src/er/versions.py PINS + S2.1 parity + installed-version parity + Splink 5 migration note"
milestone: M1
status: done
kind: code
size: S
gates: fast
depends_on: ["ER-001", "ER-003"]
spec_refs: ["s2-1", "s4-0", "s8-3", "s13"]
gap_refs: ["M25"]
provides: ["src/er/versions.py::Pin", "src/er/versions.py::PINS", "src/er/versions.py::EXTENSION_PINS", "src/er/versions.py::IMAGE_PINS", "src/er/versions.py::installed_version", "src/er/versions.py::check_installed_versions", "src/er/versions.py::code_version", "src/er/versions.py::SPLINK_MIGRATION_NOTE", "tests/unit/test_versions.py"]
consumes: ["pyproject.toml", "uv.lock", "DesignDoc.md::s2-1", "src/er/__init__.py::__version__"]
owns: ["src/er/versions.py", "tests/unit/test_versions.py"]
protected_paths: ["DesignDoc.md"]
extra_paths: []
attempts: 1
verify: "uv run pytest tests/unit/test_versions.py -q"
branch: "ticket/ER-004-src-er-versions-py-pins-s2"
commit: "16d0f48fe5c48615a5402a76fd10b87e638084a2"
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T00:05:06Z"
session: 36c43c95-016c-4c6c-a902-45f38cc95f9b
---
## Description

`src/er/versions.py` is the one place Python states what S2.1 pins, so `er doctor` (ER-022), the Dockerfile extension check (ER-007) and the compose contract test (ER-009) all read one table instead of three copies of a version string. It carries `PINS` (distribution pins), `EXTENSION_PINS` (the DuckDB extension commit hashes, with the `postgres`→`postgres_scanner` registered-name distinction that silently voids the check when missed), `IMAGE_PINS` (the three `@sha256:` service digests) and `code_version()`. Its tests assert three-way parity — the S2.1 table, the installed environment, and this module — so a dependency bump that skips the spec fails on the unit layer rather than at `er doctor` in an integration job.

## Scope

### In scope

- `Pin` dataclass: component name, pinned value, distribution name (where one exists), whether `er doctor` asserts it
- `PINS`: every S2.1 row, keyed by component name as S2.1 writes it
- `EXTENSION_PINS`: ducklake/postgres/httpfs commit hashes plus each extension's registered name as `duckdb_extensions()` reports it
- `IMAGE_PINS`: the catalog, object-store and object-store-init images with their full `@sha256:` digests
- `installed_version(dist)` via `importlib.metadata`; `check_installed_versions()` returning per-component (expected, actual, ok) rows with no I/O beyond the metadata database
- `code_version()` — the installed `er` distribution version, the value `runs.code_version` will carry
- `SPLINK_MIGRATION_NOTE` — the S13 Splink 5 row in machine-readable form

### Out of scope

- `er doctor` itself, its stdout table and its six runtime assertions (ER-022)
- Any DuckDB connection, catalog query or S3 call — this module is pure and imports no engine
- `std_version`/`survivorship_version`/`address_parser_version` resolution from config and the run-all compatibility guard (ER-011 supplies the config, ER-034 the guard) — the module gains them later
- Editing S2.1 to make a parity test pass

## Design decisions applied

Implements M25: `er doctor` had no expectation to assert because nothing was pinned. Two constraints. (1) The `postgres` extension installs under the name `postgres` but `duckdb_extensions()` reports it as `postgres_scanner`; a check written against the install name finds no row and passes silently, so `EXTENSION_PINS` MUST carry both names and consumers MUST filter on the registered one (S2.1 states this explicitly). (2) Splink 5 removes `find_matches_to_new_records`, on which D2's new-vs-corpus pass depends, so a test asserts the splink pin's major is 4: a bump to 5 must be a deliberate migration gated on T-INC-3 and T-BLK-1 (S13), not a lockfile refresh.

## Acceptance criteria

- [ ] AC1: `PINS` covers exactly the S2.1 components whose *Asserted by* cell names `er doctor`: the test parses the S2.1 table from DesignDoc.md and asserts set equality of component names in both directions, so an added S2.1 row with no `PINS` entry fails.
- [ ] AC2: For every `Pin` with a distribution name, `importlib.metadata.version(dist)` inside the synced environment equals the pinned value — `PINS["splink"].version == "4.0.16"` and `splink.__version__` agrees; the same for duckdb, dbt-core, dbt-duckdb, typer, pydantic, python-ulid, ruff, mypy, pytest, pytest-xdist, hypothesis.
- [ ] AC3: `EXTENSION_PINS` maps `ducklake`→`d8a1881e`, `postgres`→`41223e5`, `httpfs`→`827222f`, and `EXTENSION_PINS["postgres"].registered_name == "postgres_scanner"`; each value is asserted equal to its S2.1 cell parsed from the spec.
- [ ] AC4: `IMAGE_PINS` carries the three service images with 64-hex `@sha256:` digests byte-equal to their S2.1 cells.
- [ ] AC5: `check_installed_versions()` returns one row per distribution-backed pin with `ok=True` in a correctly synced environment, and returns `ok=False` (rather than raising) when handed a fabricated expectation map.
- [ ] AC6: `code_version()` returns a non-empty string equal to `importlib.metadata.version("er")`, and never raises when the package is installed in editable mode.
- [ ] AC7: `SPLINK_MIGRATION_NOTE` names `find_matches_to_new_records`, `predict_between`, `predict_within` and `src/er/matching/incremental.py`; a separate assertion pins the splink major at 4 so an upgrade to 5 turns this test red.
- [ ] AC8: `uv run mypy --strict src/er/versions.py` exits 0 and every public constant is `Final`-annotated.

## Tests

- tests/unit/test_versions.py::test_pins_cover_exactly_the_s2_1_doctor_rows
- tests/unit/test_versions.py::test_installed_versions_match_pins
- tests/unit/test_versions.py::test_extension_pins_and_registered_names
- tests/unit/test_versions.py::test_image_digests_match_s2_1
- tests/unit/test_versions.py::test_check_installed_versions_reports_mismatch
- tests/unit/test_versions.py::test_splink_major_is_4_and_migration_note_is_actionable

## Verification

```bash
uv run pytest tests/unit/test_versions.py -q
uv run mypy --strict src/er/versions.py
uv run ruff check src/er/versions.py
```

## Definition of Done

- Acceptance criteria met
- Verify command passes
- Three-way parity (S2.1 ↔ installed ↔ PINS) asserted, not assumed
- `postgres_scanner` registered-name distinction encoded
- DesignDoc.md unmodified
- Committed on main
