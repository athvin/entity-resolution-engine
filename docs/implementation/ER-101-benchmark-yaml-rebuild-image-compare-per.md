---
id: ER-101
title: "benchmark.yaml rebuild: in-image compare, per-scale runs-on/timeouts, weekly smoke cron, preflight, always-upload/teardown, SHA pins, options↔baselines coupling"
milestone: M5
status: todo
kind: code
size: M
gates: fast
depends_on: ["ER-010", "ER-099", "ER-100"]
spec_refs: ["s9", "s9-1", "s9-2", "s10-2", "s10-3", "s10-4", "s7-1", "s12"]
gap_refs: ["M24", "M23", "M25"]
provides: [".github/workflows/benchmark.yaml", "benchmarks/workflow.py::parse_benchmark_workflow", "benchmarks/workflow.py::WorkflowEnvelope", "benchmarks/workflow.py::dispatch_options", "benchmarks/workflow.py::preflight_script", "tests/unit/bench/test_workflow.py", "tests/unit/bench/test_preflight.py"]
consumes: ["benchmarks/scales.yaml", "benchmarks/scales.py", "benchmarks/report.py", "benchmarks/baselines/smoke.json", "docker/compose.yaml", "scripts/ci/actionlint.sh", ".github/workflows/ci.yaml"]
owns: [".github/workflows/benchmark.yaml", "benchmarks/workflow.py", "tests/unit/bench/test_workflow.py", "tests/unit/bench/test_preflight.py"]
protected_paths: [".github/workflows/ci.yaml", "tests/unit/test_ci_workflow.py"]
extra_paths: [".github/workflows/benchmark.yaml"]
attempts: 0
verify: "bash scripts/ci/actionlint.sh && uv run pytest tests/unit/bench/test_workflow.py tests/unit/bench/test_preflight.py -q"
branch: ""
commit: ""
spec_sha: ""
updated_at: "2026-08-14T20:02:00Z"
---
## Description

Rebuild `.github/workflows/benchmark.yaml` to the S9.2 form: `workflow_dispatch` + a single weekly cron, per-scale `runs-on` and `timeout-minutes` driven by the S10.2 envelope table, a disk/envelope preflight that exports `ER_CPU_LIMIT`/`ER_MEM_LIMIT`/`ER_DUCKDB_MEMORY_LIMIT` before Compose reads them, an in-image `report.py --compare` gate at `--fail-threshold 1.25`, and `if: always()` upload-then-teardown. Every `uses:` is pinned to a commit SHA (S9) and no runner-side Python toolchain exists — all Python runs inside `er-pipeline:ci` (S9.2, 'Everything Python runs in-image'), which is what makes the S10.4 fingerprint come from the measuring environment. The ticket also ships `benchmarks/workflow.py`, the single parser of this workflow, so that ER-102's `--validate-baselines` and this ticket's tests read the dispatch options, runner and exported envelope through one implementation rather than two greps. Closes M24 (the gate that never ran), M23 (missing permissions/concurrency/timeouts) and M25 (SHA pins) for the benchmark path.

## Scope

### In scope

- `.github/workflows/benchmark.yaml`: `on.workflow_dispatch` (`scale` choice + `repeat` string), one `schedule` cron `0 6 * * 1`, `permissions: {contents: read}`, `concurrency` keyed on the scale with `cancel-in-progress: false`, `env.SCALE`/`env.REPEAT`.
- `runs-on: ${{ (inputs.scale == '100k') && 'ubuntu-latest-8-cores' || 'ubuntu-latest' }}` and `timeout-minutes: ${{ (inputs.scale == '100k') && 120 || 40 }}`, per S10.2's runner column.
- Preflight step: `down -v --remove-orphans`, host `statvfs` free-space read, `min_free_gb` threshold and the three envelope values obtained from `docker run --rm er-pipeline:ci python benchmarks/scales.py --scale "$SCALE" --field <f>`, appended to `$GITHUB_ENV`.
- Run and compare steps via `docker compose -f docker/compose.yaml --profile bench run --rm benchmark python benchmarks/report.py …`, compare using `--baselines-dir /app/benchmarks/baselines --scale "$SCALE" --fail-threshold 1.25`.
- Upload (`if: always()`, `if-no-files-found: error`) after the compare, then teardown (`if: always()`).
- `benchmarks/workflow.py`: parse the workflow into `{dispatch_options, scheduled_scale, per-scale runner, per-scale timeout, preflight-exported env fields, uses-pins}`.
- Unit tests that (a) assert the parsed workflow against `benchmarks/scales.yaml` and the committed baseline set and (b) execute the extracted preflight shell under a stubbed `docker` binary.

### Out of scope

- Committing `10k`/`100k` baselines or widening the dispatch option list — ER-102.
- Implementing `report.py --validate-baselines` or adding the `Baseline/dispatch parity` step to `ci.yaml` — ER-102 (`ci.yaml` and its test are protected here).
- Changing `docker/compose.yaml`, `benchmarks/scales.yaml`, `benchmarks/scales.py` or `report.py` behaviour.
- Adding a `1m` dispatch option (no baseline exists; S9.2 forbids a dispatchable scale without one).
- Any PR-path trigger (`pull_request`/`push`) on this workflow — G4 and S9.2 forbid it.

## Design decisions applied

Implements gap M24/M23/M25 for the benchmark workflow under the S9.2 rules. Constraints that are easy to miss: (1) the `options:` list MUST equal the set of `benchmarks/baselines/*.json` basenames **present at this commit** (S9.2 'Dispatch options': a scale becomes dispatchable only once its baseline is committed) — so it is the committed set today, `1m` never appears, and the test asserts the equality dynamically so ER-102 can extend both sides together; (2) runner, `cpu_limit`, `mem_limit` and `duckdb_memory_limit` all come from the scale's S10.2 row and move together — the workflow encodes only the runner, the preflight exports the other three, and a 100k dispatch on the default 2-CPU envelope would make every run `NON_COMPARABLE` under S10.4; (3) `ER_DUCKDB_THREADS` is not exported here — S7.1's `x-er-env` derives it from `ER_CPU_LIMIT`, so exporting it separately would let the two drift; (4) nothing in the job may use `astral-sh/setup-uv`, `uv sync` or a host `uv run` — the runner has only preinstalled `python3`, and the disk figure is the one legitimate host-side computation; (5) `cancel-in-progress` MUST be false so a scheduled or in-flight measurement is never killed mid-run; (6) the compare step runs before the upload and both upload and teardown carry `if: always()`, so a REGRESSION/NON_COMPARABLE verdict still publishes `latest.json` and `report.md`.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/actionlint.sh` exits 0 with `.github/workflows/benchmark.yaml` present in the set of files it linted (an empty or skipped file list fails the test).
- [ ] AC2: `parse_benchmark_workflow('.github/workflows/benchmark.yaml').dispatch_options` equals `{p.stem for p in Path('benchmarks/baselines').glob('*.json')}` and does not contain `1m`; adding a stray option to a tmp copy makes `tests/unit/bench/test_workflow.py` fail.
- [ ] AC3: For every dispatchable scale, the parsed `runs-on` equals that scale's `runner` in `benchmarks/scales.yaml` and the parsed `timeout-minutes` is `120` for `100k` and `40` otherwise.
- [ ] AC4: Every `uses:` value in the workflow is a 40-character hex SHA with a trailing `# v…` comment; replacing one with a tag in a tmp copy makes the test fail naming that action.
- [ ] AC5: No step in the `bench` job references `astral-sh/setup-uv`, `uv sync`, or a bare `uv run`; every Python invocation in the job is either `docker run --rm er-pipeline:ci python …` or `docker compose … run --rm benchmark python …`.
- [ ] AC6: The compare step passes `--baselines-dir /app/benchmarks/baselines --scale "$SCALE" --fail-threshold 1.25`, its step index is lower than the upload step's, and both the upload and the teardown steps carry `if: always()` with `if-no-files-found: error` on the upload.
- [ ] AC7: `on` contains exactly `workflow_dispatch` and one `schedule` entry with cron `0 6 * * 1`; `permissions.contents` is `read`; `concurrency.cancel-in-progress` is `false`.
- [ ] AC8: Executing the preflight step's extracted shell with a stub `docker` on `PATH` that reports `min_free_gb` above the available space exits non-zero and leaves `$GITHUB_ENV` empty; with a value below it, the script exits 0 and `$GITHUB_ENV` contains exactly the three assignments `ER_CPU_LIMIT`, `ER_MEM_LIMIT`, `ER_DUCKDB_MEMORY_LIMIT` with the stub's values.

## Tests

- tests/unit/bench/test_workflow.py::test_dispatch_options_equal_committed_baselines
- tests/unit/bench/test_workflow.py::test_runner_and_timeout_match_scales_yaml
- tests/unit/bench/test_workflow.py::test_every_uses_is_sha_pinned
- tests/unit/bench/test_workflow.py::test_no_runner_side_python_toolchain
- tests/unit/bench/test_workflow.py::test_compare_is_in_image_and_precedes_upload
- tests/unit/bench/test_workflow.py::test_triggers_permissions_and_concurrency
- tests/unit/bench/test_preflight.py::test_preflight_fails_when_free_disk_below_min_free_gb
- tests/unit/bench/test_preflight.py::test_preflight_exports_scale_envelope_to_github_env

## Verification

```bash
bash scripts/ci/actionlint.sh && uv run pytest tests/unit/bench/test_workflow.py tests/unit/bench/test_preflight.py -q
uv run ruff check . && uv run ruff format --check .
bash scripts/gates.sh --ticket ER-101
```

## Definition of Done

- `.github/workflows/benchmark.yaml` rewritten; actionlint clean; every `uses:` SHA-pinned with a version comment.
- `benchmarks/workflow.py` is the only parser of the benchmark workflow in the repo (no second YAML grep in tests).
- Dispatch options equal the committed baseline set; `1m` absent.
- Preflight exports the S10.2 envelope and fails closed on insufficient disk, proven by executing the extracted shell under a stub `docker`.
- Compare runs in-image at `--fail-threshold 1.25` before an `if: always()` upload; teardown `if: always()`.
- `ci.yaml` and `tests/unit/test_ci_workflow.py` untouched.
- Both unit test files pass; `scripts/gates.sh --ticket ER-101` green with a receipt.
