#!/usr/bin/env bash
# Lint the workflows with the pinned actionlint, and enforce the S9 SHA-pin rule.
#
# The static job calls this rather than `scripts/actionlint.py` directly, because the
# rule S9 states -- "every `uses:` is pinned to the release commit SHA of the tag in
# its trailing comment" -- is not one actionlint implements. actionlint 1.7.7 exits 0
# on `actions/checkout@v4`, so a workflow linter alone would leave the pinning policy
# documented rather than enforced, and the first floating tag would reach CI unnoticed.
# The check lives here, beside the linter it complements, so `bash scripts/ci/actionlint.sh`
# is one command that answers "are these workflows acceptable?" for CI and for a
# developer alike.
#
# Nothing is fetched. The binary comes from the `actionlint-py` wheel `uv.lock` pins
# (S2.1) via `scripts/actionlint.py`, and the version that wheel installs is asserted
# against the S2.1 pin below -- a linter fetched at run time is an unpinned dependency
# policing a pinning rule, and its findings would change under the gate it guards.
#
# Usage: scripts/ci/actionlint.sh [workflow-dir]     # default: .github/workflows
# Exit codes: 0 clean; 1 a finding or an unpinned `uses:`; 2 no workflows to lint.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT" || exit 2

WORKFLOW_DIR="${1:-.github/workflows}"

# Both patterns are matched against `grep -n` output, so both allow for the line-number
# prefix it prepends; anchoring past it is what keeps `^` meaningful.
LINE_NUMBER='^[0-9]+:'
# A compliant reference in full: owner/repo, an unabbreviated 40-hex commit SHA, and a
# trailing comment naming the tag that SHA is the release of. The comment is not
# decoration -- it is what Dependabot reads to work out which release comes next, so a
# pin without one is a SHA nobody can ever bump.
PINNED_USES="${LINE_NUMBER}[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]+[A-Za-z0-9._-]+/[A-Za-z0-9._/-]+@[0-9a-f]{40}[[:space:]]+#[[:space:]]*v[0-9][A-Za-z0-9._-]*[[:space:]]*$"
ANY_USES='^[[:space:]]*(-[[:space:]]+)?uses:'

# The pinned wheel's version, compared against the S2.1 restatement of it. Printing the
# expected value alone would prove nothing, so this prints both and fails on a
# disagreement: that is what makes the banner an assertion rather than a label.
assert_pinned_version() {
    uv run python - <<'PY'
from importlib.metadata import PackageNotFoundError, version

from er.versions import PINS

pin = PINS["actionlint-py"].version
try:
    installed: str | None = version("actionlint-py")
except PackageNotFoundError:
    installed = None

print(f"actionlint: actionlint-py {installed} (S2.1 pin {pin}); nothing was fetched")
if installed != pin:
    print(
        "actionlint: the installed actionlint-py disagrees with the S2.1 pin; "
        "run 'uv sync --frozen'",
        file=__import__("sys").stderr,
    )
    raise SystemExit(1)
PY
}

if [[ ! -d "$WORKFLOW_DIR" ]]; then
    printf 'actionlint: %s is not a directory\n' "$WORKFLOW_DIR" >&2
    exit 2
fi

# bash 3.2 ships on macOS and has no `mapfile`, so the array is built by hand.
WORKFLOWS=()
while IFS= read -r workflow; do
    WORKFLOWS+=("$workflow")
done < <(find "$WORKFLOW_DIR" -maxdepth 1 -type f \( -name '*.yaml' -o -name '*.yml' \) | sort)

if [[ ${#WORKFLOWS[@]} -eq 0 ]]; then
    printf 'actionlint: no workflow files under %s\n' "$WORKFLOW_DIR" >&2
    exit 2
fi

status=0

assert_pinned_version || status=1

offenders=""
for workflow in "${WORKFLOWS[@]}"; do
    while IFS= read -r finding; do
        [[ -z "$finding" ]] && continue
        offenders="${offenders}${workflow}:${finding}"$'\n'
    done < <(grep -nE "$ANY_USES" "$workflow" | grep -vE "$PINNED_USES")
done

if [[ -n "$offenders" ]]; then
    printf 'actionlint: these `uses:` references are not pinned to a commit SHA with a\n' >&2
    printf 'trailing tag comment, which S9 requires:\n' >&2
    printf '%s' "$offenders" >&2
    status=1
fi

uv run python scripts/actionlint.py "${WORKFLOWS[@]}" || status=1

exit "$status"
