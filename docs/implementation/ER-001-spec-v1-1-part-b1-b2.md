---
id: ER-001
title: "Spec v1.1 Part A: B1/B2/B5-core, S12.1 decision lock D2–D8, S2.1, S4.0, S4.0b, S5/S5.0/S5.1/S5.2, S6, S7, S8.1, S8.2.1 + lint_spec.py"
milestone: M0
status: done
kind: spec-amendment
size: L
gates: fast
depends_on: []
spec_refs: ["s1", "s2", "s2-1", "s3", "s4", "s4-0", "s4-0b", "s5", "s5-0", "s5-1", "s5-2", "s6", "s6-1", "s7", "s7-1", "s7-2", "s7-3", "s7-4", "s8-1", "s8-2", "s8-2-1", "s9-1", "s12", "s12-1"]
gap_refs: ["B1", "B2", "B5", "M1", "M2", "M3", "M4", "M11", "M14", "M19", "M22", "M25", "M26", "MINOR-tenancy", "MINOR-event_id", "MINOR-lake-maint", "MINOR-golden_display", "NEW-S2.1/S4.0/S4.0b/S5.0/S5.1/S5.2/S8.2.1/S12.1/Observability"]
provides: ["DesignDoc.md::s2-1", "DesignDoc.md::s4-0", "DesignDoc.md::s4-0b", "DesignDoc.md::s5", "DesignDoc.md::s5-0", "DesignDoc.md::s5-1", "DesignDoc.md::s5-2", "DesignDoc.md::s6", "DesignDoc.md::s6-1", "DesignDoc.md::s7-1", "DesignDoc.md::s8-1", "DesignDoc.md::s8-2-1", "DesignDoc.md::s12-1", "scripts/lint_spec.py::lint", "scripts/lint_spec.py::main", "scripts/lint_spec.py::PART_A_SECTIONS", "scripts/lint_spec.py::PART_A_TOKENS", "scripts/lint_spec.py::FORBIDDEN", "scripts/lint_spec.py --part {a,b}", "tests/fixtures/designdoc_v1.0.md"]
consumes: []
owns: ["scripts/lint_spec.py", "tests/fixtures/designdoc_v1.0.md"]
protected_paths: []
extra_paths: ["DesignDoc.md"]
attempts: 0
verify: "python3 scripts/lint_spec.py DesignDoc.md && python3 scripts/lint_spec.py --expect-fail tests/fixtures/designdoc_v1.0.md"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Amend DesignDoc.md from the v1.0 draft to v1.1, closing the substrate-and-schema half of the gap report, and ship scripts/lint_spec.py so the amendment is machine-enforced rather than reviewed once. **Completed during the design phase, before the implementation loop existed** — the loop is forbidden from editing the spec it is graded against, so the amendment could not be one of its own tickets. Recorded here so the board is a complete account of the work and so the gap-report coverage table has an owner for these entries.

## Scope

### In scope

- S2.1 pinned-version table with literal versions, extension commit hashes and `@sha256:` image digests
- S4.0 command/flag/env/exit-code/stdout table, the `run-all` and `er correct` chains, `er init`/`er lake reset`/`er lake maintain`
- S4.0b: `:memory:` primary catalog, the verbatim SQL statement block, the Python-string substitution rule, the dbt profile keys that must equal it field-for-field
- S5 `ddl.py`-owned DDL (fourteen relations incl. `cut_edges`, `tf_lookup`, `runs`, `run_stages`, `ingest_batches`, `er_touched_entities`), S5.0 ownership/logical keys/constraint model/`record_key`/canonical pair ordering/VOLATILE_COLUMNS/GOLDEN_SURVIVABLE_COLUMNS, S5.1, S5.2
- S6 config document + S6.1 validators V1–V16 + the environment-variable list
- S7.1 compose listing, S7.2 attach sequence, S7.3 Dockerfile listing, S7.4 `run`-not-`up`
- S8.1 layers table + the session-namespace isolation contract; S8.2.1 phase vocabulary, headers, encoding rules and the three comparison helpers
- S12.1 decision lock D1–D15, each with a one-line resolution and status LOCKED
- `scripts/lint_spec.py`: `--part a|b`, forbidden patterns, required sections, required vocabulary, anchor/heading parity, S12.1 LOCKED check, version-header check

### Out of scope

- S4.1–S4.7 algorithm bodies, the S8.3 scenario table, S8.5, S10, S11, S13 rows — all ER-002 (`--part b`)
- The dbt-owned typed column listings in S5 (`golden_records`/`golden_lineage`/`golden_display`) — B4, ER-002
- Any file under `src/`, `dbt/`, `docker/` or `.github/` — the spec describes them; ER-003..ER-010 build them
- `scripts/lint_board.py` (ER-005) and `scripts/lint_metrics.py` (later milestone)

## Design decisions applied

Implements the M0 decision lock: D1 (namespace-only tenancy, MINOR-tenancy), D3 (`entity_membership` is current state, M3), D6 (`record_key`, M1), D7 (append-only `raw_records`, B2/M15), D9 (canonical pair ordering, M1), D10 (ULIDs + `IdFactory`, M7/MINOR-event_id), D12 (run metadata tables, M2), D14 (two owners, M4), D15 (S2.1 pins, M25); plus B1 (S7 substrate), B2 (no enforced constraints, logical keys), M4, M11/M14/M26 (S6 blocks and validators), M19 (S4.0), M22 (S8.1 isolation), MINOR-lake-maint and MINOR-golden_display. Two constraints are easy to miss. (1) The forbidden patterns live in the linter and MUST NOT be quoted in DesignDoc.md — a spec that names its own forbidden strings trips its own lint; S9.1 already states this and is the authority for the lint's duties. (2) Neither part may be vacuous: `--part a` MUST fail on `tests/fixtures/designdoc_v1.0.md` on its own, without help from Part B checks, because ER-002's arm is a separate command.

## Acceptance criteria

- [ ] AC1: `python3 scripts/lint_spec.py DesignDoc.md` exits 0.
- [ ] AC2: `python3 scripts/lint_spec.py --expect-fail tests/fixtures/designdoc_v1.0.md` exits 0, proving the linter is not vacuous: it must report defects against the committed v1.0 draft.
- [ ] AC3: DesignDoc.md declares Version 1.1 and contains no citation of a companion design document.
- [ ] AC4: S2.1 exists and every Pin cell is a literal version, extension commit hash or @sha256: digest — no placeholder survives.
- [ ] AC5: S2.1, S4.0, S4.0b, S5.0, S5.1, S5.2, S8.2.1 and S12.1 all exist, each preceded by its own anchor.
- [ ] AC6: S12.1 records decisions D2 through D8 and marks them LOCKED.
- [ ] AC7: Every heading carries an `<a id="sN-M"></a>` anchor immediately before it, with no duplicates.

## Tests

- scripts/lint_spec.py DesignDoc.md (positive arm)
- scripts/lint_spec.py --expect-fail tests/fixtures/designdoc_v1.0.md (non-vacuity arm)

## Verification

```bash
python3 scripts/lint_spec.py DesignDoc.md && python3 scripts/lint_spec.py --expect-fail tests/fixtures/designdoc_v1.0.md
```

## Definition of Done

- DesignDoc.md is at v1.1 and both linter arms pass
- tests/fixtures/designdoc_v1.0.md committed as the linter's negative arm
- docs/gap-report-v1.0.md committed as the rationale trail the tickets cite
- No placeholder or companion-document reference survives
