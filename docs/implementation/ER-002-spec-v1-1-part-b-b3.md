---
id: ER-002
title: "Spec v1.1 Part B: B3/B4/B5-full, S4.1–S4.7 semantics, S8.3 table with node ids, S8.5, S10.1/10.3/10.4, S11, S12, S13 rows, remaining minors"
milestone: M0
status: done
kind: spec-amendment
size: L
gates: fast
depends_on: ["ER-001"]
spec_refs: ["s4", "s4-1", "s4-1-1", "s4-2", "s4-3", "s4-3-1", "s4-3-2", "s4-3-3", "s4-3-4", "s4-3-5", "s4-4", "s4-4-1", "s4-4-2", "s4-5", "s4-5-1", "s4-5-2", "s4-5-3", "s4-5-4", "s4-5-5", "s4-5-6", "s4-6", "s4-7", "s5", "s5-0", "s8-2", "s8-3", "s8-4", "s8-5", "s9-1", "s9-2", "s10-1", "s10-2", "s10-3", "s10-4", "s10-5", "s11", "s12", "s12-1", "s13"]
gap_refs: ["B3", "B4", "B5", "M5", "M6", "M7", "M8", "M9", "M10", "M11", "M12", "M13", "M15", "M16", "M17", "M18", "M20", "M21", "M24", "M25", "MINOR-thresholds", "MINOR-milestones", "MINOR-address-parser", "MINOR-birth_date_precision", "NEW-S4.7/S8.5/Deletion"]
provides: ["DesignDoc.md::s4-1", "DesignDoc.md::s4-1-1", "DesignDoc.md::s4-2", "DesignDoc.md::s4-3-1", "DesignDoc.md::s4-3-2", "DesignDoc.md::s4-3-3", "DesignDoc.md::s4-3-4", "DesignDoc.md::s4-3-5", "DesignDoc.md::s4-4-1", "DesignDoc.md::s4-4-2", "DesignDoc.md::s4-5-1", "DesignDoc.md::s4-5-2", "DesignDoc.md::s4-5-3", "DesignDoc.md::s4-5-4", "DesignDoc.md::s4-5-5", "DesignDoc.md::s4-5-6", "DesignDoc.md::s4-6", "DesignDoc.md::s4-7", "DesignDoc.md::s8-3", "DesignDoc.md::s8-5", "DesignDoc.md::s10-3", "DesignDoc.md::s10-4", "DesignDoc.md::s11", "DesignDoc.md::s13", "scripts/lint_spec.py::PART_B_SECTIONS", "scripts/lint_spec.py::PART_B_TOKENS", "scripts/lint_spec.py::S8_3_NODE_ID_CHECK"]
consumes: ["scripts/lint_spec.py::lint", "scripts/lint_spec.py --part {a,b}", "DesignDoc.md::s5-0", "DesignDoc.md::s12-1", "tests/fixtures/designdoc_v1.0.md"]
owns: []
protected_paths: []
extra_paths: ["DesignDoc.md", "scripts/lint_spec.py"]
attempts: 0
verify: "python3 scripts/lint_spec.py DesignDoc.md && python3 scripts/lint_spec.py --expect-fail tests/fixtures/designdoc_v1.0.md"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

The algorithm, testing and benchmark half of the v1.1 amendment: S4.1-S4.7 stage semantics, the S8.3 scenario-test table with file paths and pytest node ids, S8.5 quality metrics, S10, S11 and the S13 risk rows. **Completed during the design phase together with ER-001**, then hardened through three repair passes against an independent verifier that found and closed 66 coherence defects and 16 further implementability defects. Kept as a separate id because the gap-report coverage table assigns different entries to each half.

## Scope

### In scope

- S4.1 + S4.1.1: `content_hash` normative definition, anti-join append, `--full-refresh-keys` tombstone derivation and the empty-delivery guard
- S4.2: `stg_*` mapping, greatest-`ingested_at` current-row rule, incremental configs and `on_schema_change`
- S4.3.1–4.3.5: level-token table, ordered training call sequence, TF policy and `tf_snapshot_id`, the two unioned Splink passes, gray band → `review_queue`
- S4.4/4.4.1/4.4.2: assertion lifecycle and precedence, CONTRADICTION-1, the partition-level never-cut with its total orders and `cut_protect_probability` escalation
- S4.5.1–4.5.6: affected node AND edge sets, label propagation to fixpoint with `clustering.max_iterations`, the overlap-matrix reconciler and INV-PERM, mint order, retraction/supersession, INV-EQ with its preconditions
- S4.6 survivorship rule table with the terminal `record_key ASC` tiebreak and `tiebreak_deterministic`; S4.7 error taxonomy and `--resume`
- The dbt-owned typed listings in S5 (`stg_*`, `int_std_records`, `int_blocking_keys`, `golden_records`, `golden_lineage`, `golden_display`) — B4
- S8.2 traps and ground truth, S8.3 (every row carrying file path + node id), S8.4, S8.5
- S10.1/10.2/10.3/10.4/10.5, S11, S12 milestone rows and the gating rule, S13 risks
- `--part b` check set in `scripts/lint_spec.py`, including the S8.3 node-id/file-path consistency check

### Out of scope

- Anything ER-001 owns: S2.1, S4.0, S4.0b, S5.0/5.1/5.2, S6, S7, S8.1, S8.2.1, S12.1
- Changing any locked decision — D1–D15 are LOCKED by ER-001; Part B may only cite them
- Writing code, fixtures or tests under `src/`, `dbt/`, `fixtures/` or `tests/`
- Renaming or removing existing anchors (later tickets' `spec_refs` resolve against them)

## Design decisions applied

Implements the remaining blockers and majors: B3 (D2 — two Splink passes unioned, `int_blocking_keys` never an input to scoring), B4 (literal `golden_records` column list, address as composite, `survivorship_version` in S6 `versions:`, `assembled_at` rule), B5-full (affected edge set, split fragment ordering, label-propagation bound), plus M5, M6 (D5 partition-level `never_match`), M7, M8, M9 (D4 frozen TF), M10, M11, M12, M13, M15 (D8 deletion in scope), M16, M17, M18, M20, M21, M24, MINOR-thresholds, MINOR-milestones, MINOR-address-parser, MINOR-birth_date_precision. Two constraints. (1) S5.0's `GOLDEN_SURVIVABLE_COLUMNS` was fixed by ER-001; the `golden_records` DDL written here MUST match it column-for-column and in the same order, and `--part b` MUST fail if they diverge. (2) The S13 row claiming incremental clustering "misses transitive links" is false for the S4.5 algorithm and MUST be replaced by the two real loss vectors (candidate generation and corpus-dependent TF) — the forbidden pattern already guards it.

## Acceptance criteria

- [ ] AC1: `python3 scripts/lint_spec.py DesignDoc.md` exits 0 and the required-vocabulary check finds every term the tickets cite (INV-PERM, INV-EQ, INV-SCORE, CONTRADICTION-1, cut_edges, tf_snapshot_id, and the rest).
- [ ] AC2: S4.3.4 specifies incremental scoring as two named Splink passes and states that int_blocking_keys is not an input to scoring (closes B3).
- [ ] AC3: S5 gives golden_records a literal typed column list and S5.0 materialises it as GOLDEN_SURVIVABLE_COLUMNS (closes B4).
- [ ] AC4: S4.5.1 defines the affected EDGE set as all currently-active edges among affected members, not this run's scored pairs (closes B5).
- [ ] AC5: S8.3 gives every scenario test a file path and a resolvable pytest node id.
- [ ] AC6: Every milestone in S12 gates only on relations and tests that exist by the end of that milestone.

## Tests

- scripts/lint_spec.py DesignDoc.md (positive arm)
- scripts/lint_spec.py --expect-fail tests/fixtures/designdoc_v1.0.md (non-vacuity arm)

## Verification

```bash
python3 scripts/lint_spec.py DesignDoc.md && python3 scripts/lint_spec.py --expect-fail tests/fixtures/designdoc_v1.0.md
```

## Definition of Done

- S4.1-S4.7, S8.3, S8.5, S10, S11 and S13 written and lint-clean
- All five gap-report blockers closed on the merits
- Independent verification pass completed with no remaining blocking defect
