#!/usr/bin/env bash
# Enforce the S9 action-pinning rule: every `uses:` is pinned to the release
# commit SHA of the tag in its trailing comment.
#
# WHY THIS EXISTS AT ALL. actionlint does not implement S9 — version 1.7.7 exits
# 0 on `actions/checkout@v4` — so a workflow linter alone leaves the pinning
# policy documented rather than enforced, and the first floating tag reaches CI
# unnoticed. The comment half matters as much as the SHA: Dependabot reads the
# `# vTAG` to work out which release comes next, so a pin without one is a SHA
# nobody can ever bump.
#
# WHY IT IS PURE BASH NOW. It used to shell out to `uv run python
# scripts/actionlint.py` and import `er.versions` for a pinned actionlint-py
# wheel. The Python/dbt removal took `scripts/actionlint.py`, the root
# `pyproject.toml` and `uv.lock` with it, so this script could no longer run at
# all — and nothing invoked it, so the failure was invisible. Found 2026-11-03:
# the skill claimed S9 was "enforced, not just documented" while nothing
# enforced it. The check that cannot be deleted by a dependency cleanup is the
# one with no dependencies.
#
# actionlint proper is still worth running and is NOT reimplemented here. It is
# now installed under a pin by `scripts/ci/install-actionlint.sh` and is
# MANDATORY in CI: the workflow sets ACTIONLINT_REQUIRED=1, which turns a
# missing binary into a failure instead of a skip. Without the binary and
# without that variable — a developer's laptop — the linting half still skips
# and the pin check still runs, so this script needs no toolchain locally.
#
# That ordering is deliberate. It was advisory first because the pin check had
# to be guarded immediately and an advisory linter must never be able to fail
# the pinning half. Advisory then stayed advisory long enough to never run at
# all (see the ACTIONLINT_REQUIRED branch at the bottom for how that happened).
#
# Usage: bash scripts/ci/actionlint.sh [workflow-dir]   # default .github/workflows
# Env:   ACTIONLINT_REQUIRED=1 — a missing actionlint binary is a failure (CI)
# Exit:  0 clean · 1 an unpinned `uses:`, an actionlint finding, or required-and-absent · 2 nothing to lint
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT" || exit 2
WORKFLOW_DIR="${1:-.github/workflows}"

# Both patterns match against `grep -n` output, so both allow for the line-number
# prefix it prepends; anchoring past it is what keeps `^` meaningful.
LINE='^[0-9]+:'
PINNED="${LINE}[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]+[A-Za-z0-9._-]+/[A-Za-z0-9._/-]+@[0-9a-f]{40}[[:space:]]+#[[:space:]]*v[0-9][A-Za-z0-9._-]*[[:space:]]*$"
ANY='^[[:space:]]*(-[[:space:]]+)?uses:'

if [[ ! -d "$WORKFLOW_DIR" ]]; then
    printf 'actionlint: %s is not a directory\n' "$WORKFLOW_DIR" >&2
    exit 2
fi

# bash 3.2 ships on macOS and has no `mapfile`, so build the array by hand.
# `.lock.yml` files are included deliberately: they are `gh aw compile` output,
# which pins for you, but output nobody checks is output that can regress.
WORKFLOWS=()
while IFS= read -r wf; do WORKFLOWS+=("$wf"); done < <(
    find "$WORKFLOW_DIR" -maxdepth 1 -type f \( -name '*.yaml' -o -name '*.yml' \) | sort
)

if [[ ${#WORKFLOWS[@]} -eq 0 ]]; then
    printf 'actionlint: no workflow files under %s\n' "$WORKFLOW_DIR" >&2
    exit 2
fi

status=0
offenders=""
examined=0

for wf in "${WORKFLOWS[@]}"; do
    n_uses=$(grep -cE "$ANY" "$wf" || true)
    examined=$((examined + ${n_uses:-0}))
    while IFS= read -r finding; do
        [[ -z "$finding" ]] && continue
        offenders="${offenders}${wf}:${finding}"$'\n'
    done < <(grep -nE "$ANY" "$wf" | grep -vE "$PINNED")
done

# A GATE MUST NOT BE ABLE TO PASS BY EXAMINING NOTHING. If the `uses:` pattern
# ever stops matching — a formatting change, a bad edit here — every workflow
# trivially "passes" and the policy silently lapses, which is exactly the state
# this script was found in. Zero matches across the whole tree is therefore a
# failure, not a clean run.
if [[ "$examined" -eq 0 ]]; then
    printf 'actionlint: examined %d workflow files and found NO `uses:` lines at all.\n' "${#WORKFLOWS[@]}" >&2
    printf 'That means the detection pattern is broken, not that the tree is clean.\n' >&2
    exit 1
fi

if [[ -n "$offenders" ]]; then
    printf 'actionlint: these `uses:` references are not pinned to a commit SHA with a\n' >&2
    printf 'trailing `# vTAG` comment, which S9 requires:\n' >&2
    printf '%s' "$offenders" >&2
    status=1
else
    printf 'actionlint: %d `uses:` references across %d workflow files, all S9-pinned\n' \
        "$examined" "${#WORKFLOWS[@]}"
fi

# actionlint proper runs over the HAND-WRITTEN workflows only.
#
# It must not lint `gh aw compile` output. Measured 2026-11-04: actionlint 1.7.7
# reports 40+ findings across the five generated files and not one is a defect —
# `runs-on: ubuntu-slim`, `concurrency.queue: max`, `job.check_run_id`,
# `job.workflow_ref` and `permissions.vulnerability-alerts` are all real GitHub
# features newer than the linter's tables. Linting them would make this gate
# permanently red, which teaches people to ignore it: the same defect as a gate
# that stays green when it matters.
#
# Generated-ness is read from each file's own header marker rather than from a
# filename pattern, so the next gh-aw workflow is excluded without anyone
# remembering to add it. The S9 pin check above deliberately still covers them —
# `gh aw compile` pins for you, but output nobody checks is output that can
# regress.
HANDWRITTEN=()
for wf in "${WORKFLOWS[@]}"; do
    if head -5 "$wf" | grep -qiE 'generated|DO NOT EDIT|gh aw'; then continue; fi
    HANDWRITTEN+=("$wf")
done

if command -v actionlint >/dev/null 2>&1; then
    if [[ ${#HANDWRITTEN[@]} -eq 0 ]]; then
        # Every file looked generated. Either the marker test is wrong or the
        # tree is, and both deserve a failure rather than a silent pass.
        printf 'actionlint: every workflow looks generated; nothing was linted\n' >&2
        status=1
    else
        printf 'actionlint: running %s over %d hand-written workflows (%d generated, skipped)\n' \
            "$(actionlint --version 2>/dev/null | head -1)" \
            "${#HANDWRITTEN[@]}" "$(( ${#WORKFLOWS[@]} - ${#HANDWRITTEN[@]} ))"
        actionlint "${HANDWRITTEN[@]}" || status=1
    fi
elif [[ "${ACTIONLINT_REQUIRED:-0}" == "1" ]]; then
    # THE HOLE THIS CLOSES. The branch below is correct for a developer who has
    # not installed actionlint, and catastrophic in CI: it prints a line nobody
    # reads and exits 0, so "the linter never ran" is indistinguishable from
    # "the linter found nothing". That was the real state from the day the
    # Python removal broke the install until 2026-11-07 — `taiki-e/install-action`
    # could never install actionlint (not a Rust crate, absent from its TOOLS.md)
    # and `continue-on-error: true` hid it, so this gate's linting half had never
    # executed once while reporting success.
    #
    # So CI sets ACTIONLINT_REQUIRED=1 and absence becomes a failure. The
    # installer is `scripts/ci/install-actionlint.sh`; if it ran, the binary is
    # here, and if it did not, that is the thing to be told about.
    printf 'actionlint: REQUIRED but not on PATH.\n' >&2
    printf 'ACTIONLINT_REQUIRED=1 means the linting half must run, and it did not.\n' >&2
    printf 'scripts/ci/install-actionlint.sh should have installed it — check that step.\n' >&2
    printf 'The S9 pin check above still ran and its verdict stands.\n' >&2
    status=1
else
    printf 'actionlint: binary not on PATH; S9 pin check ran, workflow linting skipped\n'
    printf '(set ACTIONLINT_REQUIRED=1 to make this a failure, as CI does)\n'
fi

exit "$status"
