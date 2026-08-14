---
id: ER-102
title: "Baselines 10k/100k + --validate-baselines + baselines/README.md + flip the scheduled scale to 10k"
milestone: M5
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-101"]
spec_refs: ["s8-1", "s9-1", "s9-2", "s10-2", "s10-3", "s10-4", "s12", "s13"]
gap_refs: ["M24", "MINOR-milestones"]
provides: ["benchmarks/baselines/10k.json", "benchmarks/baselines/100k.json", "benchmarks/baselines/README.md", "benchmarks/report.py::validate_baselines", "workflow-step:.github/workflows/ci.yaml#static:Baseline/dispatch parity", "tests/unit/bench/test_baselines.py"]
consumes: ["benchmarks/report.py", "benchmarks/workflow.py::parse_benchmark_workflow", "benchmarks/scales.yaml", "benchmarks/scales.py", "benchmarks/bench_result.schema.json", "benchmarks/baselines/smoke.json", ".github/workflows/benchmark.yaml", ".github/workflows/ci.yaml"]
owns: ["benchmarks/baselines/10k.json", "benchmarks/baselines/100k.json", "benchmarks/baselines/README.md", "tests/unit/bench/test_baselines.py"]
protected_paths: ["tests/unit/bench/test_workflow.py", "tests/unit/bench/test_preflight.py"]
extra_paths: [".github/workflows/ci.yaml", ".github/workflows/benchmark.yaml"]
attempts: 0
verify: "uv run pytest tests/unit/bench/test_baselines.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Commit the `10k` and `100k` baselines produced by `report.py --write-baseline` from real dispatched runs, implement `report.py --validate-baselines` (the S9.1 static-job lint that couples the dispatch option list to the committed baseline set and each dispatchable scale's runner/envelope to its S10.2 row), add the `Baseline/dispatch parity` step to `ci.yaml`'s static job as S12 M5 requires, and document the baseline lifecycle in `benchmarks/baselines/README.md`. With three baselines committed the weekly cron is moved from `smoke` to `10k`, which shares `smoke`'s runner and envelope (S10.2) so the 40-minute timeout and the `runs-on` expression are unchanged. Closes M24's 'no defined bootstrap path' and the MINOR-milestones item that M5 must ship all three baselines before the static job can go green.

## Scope

### In scope

- `benchmarks/report.py --validate-baselines --baselines-dir DIR --workflow PATH`: baseline file set == workflow dispatch options; for each dispatchable scale, the workflow's runner and exported envelope == that scale's `scales.yaml` row (S9.1, S10.3 flag table).
- Committing `benchmarks/baselines/10k.json` and `benchmarks/baselines/100k.json`, each produced by `--write-baseline` from a run that satisfies the S10.4 comparability rule.
- Extending `.github/workflows/benchmark.yaml`'s `options:` to `[smoke, 10k, 100k]` and flipping every `inputs.scale || …` fallback (env.SCALE, concurrency group, and any other) from `smoke` to `10k` in one edit.
- `benchmarks/baselines/README.md`: the `--write-baseline` bootstrap, the per-scale S10.2 envelope each baseline was measured under, the NON_COMPARABLE refusal, and the rule that a baseline changes only via a reviewed PR.
- The `Baseline/dispatch parity` step in `ci.yaml`'s static job, with the three flags S9.1 shows.
- `tests/unit/bench/test_baselines.py` validating the committed JSONs, the coupling lint (positive and negative arms) and the two workflow edits.

### Out of scope

- Rewriting `benchmarks/baselines/smoke.json` or changing `--run`/`--compare`/`--repeat` semantics (ER-099).
- Restructuring `benchmark.yaml` beyond the option list and the scheduled-scale fallbacks (ER-101 owns its shape; its two test files are protected here).
- Adding a `1m` baseline or option (deferred; no runner class).
- Relaxing `--fail-threshold`, the 0.15 CV bound, or any S10.4 comparability condition to make a measured run committable.
- Editing `DesignDoc.md` (S9.2's Cadence sentence still reads `smoke`; reconciling it is spec-amendment work, not this ticket).

## Design decisions applied

Implements M24's baseline bootstrap plus the MINOR-milestones M5 requirement. Hard constraints: (1) **never commit a baseline measured outside its S10.2 row** — `--write-baseline` must refuse a `NON_COMPARABLE` run (S10.4), and S12/M5 plus the S13 row make `ubuntu-latest-8-cores` an environmental precondition for `100k`; if that runner label is not available to this repository, block the ticket with class `environment` naming the label rather than committing a `100k` baseline from a 2-vCPU runner or demoting the scale (demotion changes S10.2 cells and is spec work). (2) Every fallback expression that spells the scheduled scale must be flipped together — `env.SCALE`, `env.REPEAT`'s neighbour `concurrency.group`, and any other `inputs.scale || 'smoke'` — otherwise a scheduled run measures `10k` while grouping under `smoke` and two schedules can overlap. (3) The board mandates the `10k` cron once its baseline exists; S9.2's Cadence bullet and the cron comment still say `smoke`, so record the divergence in the commit body and in `baselines/README.md` for the next spec review — do not edit `DesignDoc.md`. (4) `--validate-baselines` must read the workflow through `benchmarks/workflow.py::parse_benchmark_workflow` (ER-101), not a second YAML grep. (5) The static job's step list is owned by S9.1; add exactly the `Baseline/dispatch parity` step it shows and nothing else.

## Acceptance criteria

- [ ] AC1: `uv run python benchmarks/report.py --validate-baselines --baselines-dir benchmarks/baselines --workflow .github/workflows/benchmark.yaml` exits 0 on the committed tree; against a tmp tree with `10k.json` removed it exits non-zero and its stderr names `10k`.
- [ ] AC2: Against a tmp copy of the workflow whose `100k` runner is rewritten to `ubuntu-latest`, `--validate-baselines` exits non-zero naming `100k` and the runner mismatch; against one whose preflight exports a `mem_limit` different from `scales.yaml`, it exits non-zero naming the field.
- [ ] AC3: For each of `smoke.json`, `10k.json`, `100k.json`: the fingerprint's `scale` equals the filename stem, cgroup CPU quota == that scale's `cpu_limit`, cgroup `memory.max` == `mem_limit`, `ER_DUCKDB_THREADS` == `cpu_limit`, `ER_DUCKDB_MEMORY_LIMIT` == `duckdb_memory_limit`, verdict is not `NON_COMPARABLE`, `repeat >= 3`, and every per-phase `wall_ms` CV <= 0.15.
- [ ] AC4: Each committed baseline validates against `benchmarks/bench_result.schema.json` and contains non-null `incremental_ratio` and `blocking_recall`.
- [ ] AC5: `parse_benchmark_workflow('.github/workflows/benchmark.yaml')` reports `dispatch_options == {'smoke','10k','100k'}` and `scheduled_scale == '10k'`, and no `inputs.scale || 'smoke'` fallback remains anywhere in the file.
- [ ] AC6: `.github/workflows/ci.yaml`'s `static` job contains a step named `Baseline/dispatch parity` running `benchmarks/report.py --validate-baselines` with `--baselines-dir benchmarks/baselines` and `--workflow .github/workflows/benchmark.yaml`, and that is the only `--validate-baselines` invocation in either workflow.
- [ ] AC7: `benchmarks/baselines/README.md` names, for each committed baseline, the scale's four S10.2 envelope values, and contains the literal `--write-baseline` bootstrap command; a test asserts the README's per-scale envelope values equal `scales.yaml`.
- [ ] AC8: `uv run pytest tests/unit/bench/test_workflow.py tests/unit/bench/test_preflight.py -q` still passes with those files unmodified after the option-list and cron edits.

## Tests

- tests/unit/bench/test_baselines.py::test_validate_baselines_accepts_committed_tree
- tests/unit/bench/test_baselines.py::test_validate_baselines_rejects_missing_baseline
- tests/unit/bench/test_baselines.py::test_validate_baselines_rejects_runner_envelope_mismatch
- tests/unit/bench/test_baselines.py::test_committed_baselines_are_comparable_under_s10_4
- tests/unit/bench/test_baselines.py::test_committed_baselines_match_result_schema_and_quality_keys
- tests/unit/bench/test_baselines.py::test_scheduled_scale_is_10k_everywhere
- tests/unit/bench/test_baselines.py::test_ci_static_job_has_baseline_dispatch_parity_step
- tests/unit/bench/test_baselines.py::test_baselines_readme_documents_envelopes

## Verification

```bash
uv run pytest tests/unit/bench/test_baselines.py -q
uv run python benchmarks/report.py --validate-baselines --baselines-dir benchmarks/baselines --workflow .github/workflows/benchmark.yaml
uv run pytest tests/unit/bench/test_workflow.py tests/unit/bench/test_preflight.py -q
bash scripts/ci/actionlint.sh
bash scripts/gates.sh --ticket ER-102
```

## Definition of Done

- `10k.json` and `100k.json` committed, each produced by `--write-baseline` from a dispatched run inside its S10.2 envelope (fingerprint proves it).
- `--validate-baselines` implemented in `report.py`, reading the workflow through `benchmarks/workflow.py`; positive and negative arms tested.
- Dispatch options are `[smoke, 10k, 100k]`; every scheduled-scale fallback reads `10k`; `1m` still absent.
- `Baseline/dispatch parity` step present in `ci.yaml`'s static job and green locally.
- `benchmarks/baselines/README.md` documents the bootstrap, the envelopes and the reviewed-PR rule, and notes the S9.2 Cadence divergence.
- ER-101's workflow/preflight tests pass unmodified.
- `scripts/gates.sh --ticket ER-102` green with a receipt.
