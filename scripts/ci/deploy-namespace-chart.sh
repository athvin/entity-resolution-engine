#!/usr/bin/env bash
# Install or upgrade the per-namespace chart, and the StorageClass it needs.
#
# `ebs-gp3.yaml` is CLUSTER-SCOPED and so cannot live in the namespace chart;
# applying it here keeps the ordering explicit — a chart whose PVCs name a
# StorageClass that does not exist yet stays Pending past `--wait`'s timeout
# and fails as a chart timeout rather than as a missing StorageClass.
#
# `--wait`: the colour steps that follow read live Service selectors, so the
# namespace's objects have to exist before any of them runs.
#
# Inputs (env): CI_NAMESPACE.
# Run: bash scripts/ci/deploy-namespace-chart.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

: "${CI_NAMESPACE:?CI_NAMESPACE must be set}"

kubectl apply -f infra/k8s/storage/ebs-gp3.yaml
helm upgrade --install "$CI_NAMESPACE" infra/k8s/charts/er-dev-namespace \
	--namespace default --set "developer=$CI_NAMESPACE" --wait
