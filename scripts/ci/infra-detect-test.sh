#!/usr/bin/env bash
# A registry-only change must plan/apply IAM AND render/deploy the chart.
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT
cd "$fixture_root"
git -c init.defaultBranch=main init -q
git -c user.name=fixture -c user.email=fixture@example.invalid commit --allow-empty -qm baseline

check_path() {
    local path="$1" want_infra="$2" want_charts="$3" before after
    before="$(git rev-parse HEAD)"
    mkdir -p "$(dirname "$path")"
    printf 'fixture\n' > "$path"
    git add -- "$path"
    git -c user.name=fixture -c user.email=fixture@example.invalid commit -qm fixture
    after="$(git rev-parse HEAD)"
    : > "$fixture_root/output"
    EVENT=push PUSH_BEFORE="$before" PUSH_AFTER="$after" GITHUB_OUTPUT="$fixture_root/output" \
        bash "$repo_root/scripts/ci/infra-detect.sh" > "$fixture_root/log"
    grep -qx "infra=$want_infra" "$fixture_root/output"
    grep -qx "charts=$want_charts" "$fixture_root/output"
}
check_path infra/k8s/charts/er-platform/tenant-runners.json true true
check_path infra/k8s/charts/er-platform/values.yaml false true
check_path infra/terraform/envs/dev/example.tf true false
check_path scripts/ci/tenant-runners-check.py false true
check_path scripts/ci/infra-detect-test.sh true false
check_path scripts/ci/runner-isolation-pod.sh false true
check_path scripts/ci/runner-isolation.sh false true
check_path scripts/ci/runner-isolation-test.py false true
check_path .github/workflows/runner-isolation.yml false true
printf 'infra detector: registry and neighboring input boundaries pass\n'
check_path infra/k8s/duckfleet/dev/bootstrap.yaml false true
check_path scripts/ci/duckfleet-boundary-test.py false true
