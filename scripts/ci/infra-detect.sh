#!/usr/bin/env bash
# infra.yml's detect step: did terraform inputs change, did the charts?
#
# Inputs (env): EVENT, PR_BASE, PR_HEAD, PUSH_BEFORE, PUSH_AFTER, MQ_BASE,
# MQ_HEAD. Outputs ($GITHUB_OUTPUT): infra, charts.
#
# THE PUSH PATH FAILS SAFE, and the asymmetry is deliberate. A wrongly-skipped
# *plan* is cosmetic — the PR simply shows nothing. A wrongly-skipped *apply*
# silently diverges the account from its configuration until the next infra PR
# happens to notice. So only a confidently-computed empty diff may skip: an
# absent, all-zero or unreachable before-SHA (a force-push, a freshly-created
# branch) means "assume infra changed" rather than "assume it did not".
case "$EVENT" in
  pull_request)
    range="$PR_BASE...$PR_HEAD"
    ;;
  merge_group)
    range="$MQ_BASE...$MQ_HEAD"
    ;;
  push)
    if [ -z "$PUSH_BEFORE" ] \
       || [ "$PUSH_BEFORE" = "0000000000000000000000000000000000000000" ] \
       || ! git cat-file -e "${PUSH_BEFORE}^{commit}" 2>/dev/null; then
      echo "cannot compute the pushed range (before='$PUSH_BEFORE') — assuming infra changed"
      echo "infra=true" >> "$GITHUB_OUTPUT"
      echo "charts=true" >> "$GITHUB_OUTPUT"
      exit 0
    fi
    range="$PUSH_BEFORE...$PUSH_AFTER"
    ;;
  *)
    echo "event '$EVENT' carries no diff — assuming infra changed"
    echo "infra=true" >> "$GITHUB_OUTPUT"
    echo "charts=true" >> "$GITHUB_OUTPUT"
    exit 0
    ;;
esac

# `grep -c` reads all its input, unlike `grep -q`, which closes the
# pipe early and can SIGPIPE `git diff` under `pipefail` into a flaky
# false skip; `|| true` absorbs grep's exit-1-on-zero-matches under
# `set -e`. Same idiom and same reason as rust.yml's detect job.
#
# `scripts/ci/tf-*` is in the pattern on purpose: the apply guard and
# the comment assembler live there, and a fix to one of them that did
# not run this workflow would ship untested.
changed="$(git diff --name-only "$range")"
printf 'changed paths in %s:\n%s\n' "$range" "$changed"
n="$(printf '%s\n' "$changed" \
      | grep -cE '^(infra/terraform/|infra/k8s/charts/er-platform/tenant-runners\.json$|scripts/ci/tf-|scripts/ci/infra-detect(-test)?\.sh$|scripts/ci/gate-all-legs\.sh$|\.github/workflows/infra\.yml$)' || true)"
if [ "${n:-0}" -gt 0 ]; then
  echo "infra=true" >> "$GITHUB_OUTPUT"
else
  echo "infra=false" >> "$GITHUB_OUTPUT"
fi

# The HELM half, computed separately because it shares none of the
# terraform tiers' inputs, EXCEPT tenant-runners.json: the registry feeds
# both Terraform identities and the chart. Other chart changes must not trigger a
# `terraform plan`, and a terraform change must not pay for a chart
# render. Same allowlist direction, same `grep -c`, same reason.
#
# `scripts/ci/charts-check.py` is in the pattern for the reason
# `scripts/ci/tf-*` is in the one above: a fix to the gate that did
# not run the gate would ship untested.
#
# `helm-lint.sh`, `install-chart-tools.sh` and `install-helm.sh` are
# named for exactly that reason, and they are here because extracting
# them from infra.yml would otherwise have REMOVED them from this
# pattern's sight: they were inline in `.github/workflows/infra.yml`,
# which the last alternative matches, so the tier re-ran on any edit
# to them. As files they match nothing above. That is the
# `paths:`-filter trap in CLAUDE.md — a change to a gate no longer
# triggering the gate it defines — arriving through a refactor rather
# than through a filter.
#
# `install-helm.sh` owns the helm pin for three workflows, so a bump
# to it must render the charts with the helm that will deploy them.
c="$(printf '%s\n' "$changed" \
      | grep -cE '^(infra/k8s/duckfleet/|scripts/ci/duckfleet-boundary-test\.py$|infra/k8s/charts/|scripts/ci/charts-check\.py$|scripts/ci/tenant-runners-check\.py$|scripts/ci/runner-isolation[^/]*\.(sh|py)$|\.github/workflows/runner-isolation\.yml$|scripts/ci/charts-render-validate\.sh$|scripts/ci/helm-lint\.sh$|scripts/ci/install-chart-tools\.sh$|scripts/ci/install-helm\.sh$|\.github/workflows/infra\.yml$)' || true)"
if [ "${c:-0}" -gt 0 ]; then
  echo "charts=true" >> "$GITHUB_OUTPUT"
else
  echo "charts=false" >> "$GITHUB_OUTPUT"
fi
