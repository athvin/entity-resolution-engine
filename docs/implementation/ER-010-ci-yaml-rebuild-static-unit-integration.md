---
id: ER-010
title: "ci.yaml rebuild: static/unit/integration, permissions, concurrency, timeouts, SHA-pinned actions, quoted cache-to, actionlint + spec lint + board lint"
milestone: M1
status: in_progress
kind: code
size: M
gates: fast
depends_on: ["ER-003", "ER-005", "ER-008", "ER-009"]
spec_refs: ["s2-1", "s3", "s8-1", "s9", "s9-1", "s12"]
gap_refs: ["M23", "M25", "M24"]
provides: [".github/workflows/ci.yaml", ".github/dependabot.yml", "scripts/ci/actionlint.sh", "tests/unit/test_ci_workflow.py", "ci-job:static", "ci-job:unit", "ci-job:integration"]
consumes: ["scripts/actionlint.py::main", "scripts/lint_spec.py::main", "scripts/lint_board.py::main", "docker/Dockerfile", "docker/compose.yaml", "scripts/ci/itest.sh", "dbt-target:mem", "pyproject.toml", "uv.lock"]
owns: [".github/workflows/ci.yaml", ".github/dependabot.yml", "scripts/ci/actionlint.sh", "tests/unit/test_ci_workflow.py"]
protected_paths: ["DesignDoc.md", "docker/compose.yaml", "scripts/ci/itest.sh"]
extra_paths: [".github/workflows/ci.yaml", ".github/dependabot.yml"]
attempts: 1
verify: "bash scripts/ci/actionlint.sh && uv run pytest tests/unit/test_ci_workflow.py -q"
branch: "ticket/ER-010-ci-yaml-rebuild-static-unit-integration"
commit: ""
spec_sha: "28d8d8e366a7b49b"
updated_at: "2026-08-15T04:55:40Z"
session: 8835c27d-492a-4392-a6ca-f572338a9ec1
---
## Description

Rebuild the PR-path workflow to S9.1: three jobs (`static`, `unit` in parallel; `integration` with `needs: [static, unit]`), `permissions: {contents: read}`, a `concurrency` group that cancels only on pull requests, `timeout-minutes` 10/10/25, every `uses:` pinned to a commit SHA with its tag in a trailing comment, and `cache-to: "type=gha,mode=max"` quoted so the comma does not terminate the flow mapping. The static job runs ruff, `mypy --strict src/er`, actionlint, the spec lint and the board lint before `dbt deps`/`dbt parse --target mem`; the integration job builds `er-pipeline:ci`, resets the substrate, runs the suite through Compose and always tears down and uploads with `if-no-files-found: error`. `scripts/ci/actionlint.sh` wraps the wheel-shipped binary so the workflow linter is itself pinned.

## Scope

### In scope

- `.github/workflows/ci.yaml` — the three jobs, their step order, timeouts, permissions, concurrency and artifact uploads
- `scripts/ci/actionlint.sh` — invoke `uv run python scripts/actionlint.py` over `.github/workflows/`, propagate the exit status, download nothing
- `.github/dependabot.yml` — `github-actions` ecosystem so SHA bumps arrive as reviewed PRs (S9)
- `tests/unit/test_ci_workflow.py` — YAML-level assertions on the workflow, runnable without GitHub

### Out of scope

- `benchmark.yaml` and the `Baseline/dispatch parity` static step — both M5 (ER-101/ER-102); S9.1 says explicitly the parity step is not in the ci.yaml M1 delivers
- `scripts/lint_metrics.py` — its S8.5 subject does not exist yet, so the step is added with the metrics implementation, not here
- Editing `docker/compose.yaml` or `scripts/ci/itest.sh` (ER-009 owns them; this ticket only calls them)
- Branch-protection settings, which are a repository setting rather than a file

## Design decisions applied

Implements M23 (no permissions/concurrency/timeouts, serialised jobs, unquoted `cache-to`, stale `setup-uv`, no junit), M25 (SHA-pinned actions enforced by actionlint) and M24's PR-path half (uploads that fail loudly when empty). Three constraints. (1) A workflow step that references a script the repository does not yet have turns every PR red for three milestones — so the M5-only steps stay out and a test asserts that every `run:` step references only files that exist. (2) `cache-to` MUST be a quoted scalar: unquoted, YAML ends the flow-mapping entry at the comma and the buildx cache is silently inline-only. (3) `actionlint` MUST come from the pinned `actionlint-py` wheel (S2.1) — a binary downloaded at CI time is an unpinned dependency policing a pinning rule.

## Acceptance criteria

- [ ] AC1: `bash scripts/ci/actionlint.sh` exits 0 against the committed workflows and its output names the actionlint version equal to the S2.1 pin; the script contains no `curl`, `wget` or `go install`.
- [ ] AC2: On a temp copy of `ci.yaml` whose first `uses:` is rewritten to `actions/checkout@v4`, `scripts/ci/actionlint.sh` exits non-zero — the SHA-pin rule is enforced, not documented.
- [ ] AC3: `tests/unit/test_ci_workflow.py` asserts `permissions: {contents: read}`, `concurrency.group == '${{ github.workflow }}-${{ github.ref }}'` with `cancel-in-progress: ${{ github.event_name == 'pull_request' }}`, `timeout-minutes` of 10/10/25, and that `needs: [static, unit]` appears on `integration` only (static and unit run in parallel).
- [ ] AC4: Every `uses:` value matches `^[\w.-]+/[\w.-]+@[0-9a-f]{40}$` and is followed by a trailing comment naming its tag; `cache-to` parses to the exact string `type=gha,mode=max`.
- [ ] AC5: Every `run:` step referencing a repository path (`scripts/…`, `benchmarks/…`, `docker/…`) names a file that exists — so the absence of the M5-only `lint_metrics.py` and `--validate-baselines` steps is enforced mechanically rather than remembered.
- [ ] AC6: The static job's steps are, in order: `uv sync --frozen`, `ruff check .`, `ruff format --check .`, `mypy --strict src/er`, actionlint, `lint_spec.py DesignDoc.md`, `lint_board.py`, `dbt deps`, `dbt parse --target mem`; the unit job runs `dbt compile --target mem` and `pytest tests/unit -n auto -q --junitxml=artifacts/junit-unit.xml` after `mkdir -p artifacts`.
- [ ] AC7: The integration job builds with `docker/build-push-action` (`load: true`, `tags: er-pipeline:ci`), runs `down -v --remove-orphans` before the suite and again in an `if: always()` step, and both upload steps use `if: always()` with `if-no-files-found: error`.
- [ ] AC8: `.github/dependabot.yml` declares the `github-actions` ecosystem for `/` with a weekly schedule, and actionlint reports no findings on it or on either workflow file.

## Tests

- tests/unit/test_ci_workflow.py::test_permissions_concurrency_and_timeouts
- tests/unit/test_ci_workflow.py::test_all_uses_are_sha_pinned_with_tag_comments
- tests/unit/test_ci_workflow.py::test_cache_to_is_a_quoted_scalar
- tests/unit/test_ci_workflow.py::test_run_steps_reference_existing_files
- tests/unit/test_ci_workflow.py::test_static_job_step_order
- tests/unit/test_ci_workflow.py::test_integration_job_teardown_and_uploads
- tests/unit/test_ci_workflow.py::test_job_graph_runs_static_and_unit_in_parallel

## Verification

```bash
bash scripts/ci/actionlint.sh && uv run pytest tests/unit/test_ci_workflow.py -q
uv run python scripts/lint_spec.py DesignDoc.md && uv run python scripts/lint_board.py
bash scripts/gates.sh --fast
```

## Definition of Done

- Acceptance criteria met
- Verify command passes
- Unpinned-`uses:` negative arm proven against a temp copy
- No workflow step references a file that does not exist
- M5-only steps deliberately absent and documented in the workflow comments
- dependabot.yml committed for the github-actions ecosystem
- compose.yaml and itest.sh unmodified
- Committed on main
