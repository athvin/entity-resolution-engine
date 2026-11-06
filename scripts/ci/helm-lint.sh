#!/usr/bin/env bash
# `helm lint` both charts in infra/k8s/charts.
#
# The `--set` values are placeholders, not configuration: `helm lint` still
# evaluates templates, so a chart whose values are required would fail on the
# missing value rather than on anything about the chart. `fs-0`, `fsap-0` and
# `developer=ci` exist only to get past that and are never deployed.
#
# Run: bash scripts/ci/helm-lint.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

helm lint infra/k8s/charts/er-platform \
	--set efs.fileSystemId=fs-0 --set efs.accessPointId=fsap-0
helm lint infra/k8s/charts/er-dev-namespace --set developer=ci
