#!/usr/bin/env bash
# Diff the control-plane OpenAPI document against main's and report breaking
# schema and operation changes for both internal and public audiences.
#
# IT CANNOT FAIL THE REQUIRED JOB IT RIDES ON, and that is structural rather
# than incidental: every stage is `|| exit 0` or `|| true`. No `set -e` here
# for the same reason — an advisory gate that could go red would make the
# `entities` job fail on an API report.
#
# It is advisory until the first external org key, after which the release
# pipeline runs `er-openapi-check --strict`.
#
# PR-ONLY, which the steps always implied: it diffs against `origin/main`, so
# on a push to main there is no base and it exits 0 having examined nothing.
# The `if:` on the call site states that, because running it there was ~10
# minutes to print nothing.
#
# Run: bash scripts/ci/api-breaking-gate.sh  (from the repo root; the script
# cds, and enters rust/ itself for the cargo invocation)
set -uo pipefail

cd "$(dirname "$0")/../.."

git fetch --no-tags --depth=1 origin main || exit 0
base_dir="$(mktemp -d)" || exit 0
trap 'rm -rf "$base_dir"' EXIT
for audience in internal public; do
    artifact=openapi.json
    if [ "$audience" = public ]; then
        artifact=openapi.public.json
    fi
    printf '\nAPI compatibility: %s audience (advisory)\n' "$audience"
    if ! git cat-file -e "origin/main:frontend/src/lib/api/$artifact" 2>/dev/null; then
        printf 'No %s baseline on origin/main; initial publication, no comparison.\n' "$audience"
        continue
    fi
    git show "origin/main:frontend/src/lib/api/$artifact" > "$base_dir/$artifact" || continue
    (cd rust && cargo run -q -p er-server --bin er-openapi-check -- "$base_dir/$artifact" --audience "$audience") || true
done
