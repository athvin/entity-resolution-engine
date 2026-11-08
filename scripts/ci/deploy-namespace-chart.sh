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
# prod (DEPLOY_ENV=prod) differs twice, and both are the chart behaving as
# designed rather than a fork: `bootstrap.enabled=false` because prod
# deliberately has no per-namespace Postgres role (envs/prod/secrets.tf — the
# master role serves the one `er` database, and the job's
# `er/prod/app/postgres` secret does not exist to resolve), and
# `webIngressFromVpc.enabled=true` because prod's ALB reaches pod IPs from the
# VPC — the hole default-deny-ingress otherwise blackholes. The two flags are
# the chart's own seams; the quota and LimitRange values are NOT overridden —
# the dev arithmetic (limits.cpu 40, ephemeral 900Gi, jobs 30) is a per-
# namespace ceiling against the same runner classes and instance shapes, and
# prod gets a different number only with a measurement, not a guess.
#
# Inputs (env): CI_NAMESPACE; DEPLOY_ENV (dev|prod, default dev).
# Run: bash scripts/ci/deploy-namespace-chart.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

: "${CI_NAMESPACE:?CI_NAMESPACE must be set}"
DEPLOY_ENV="${DEPLOY_ENV:-dev}"

kubectl apply -f infra/k8s/storage/ebs-gp3.yaml

args=(--namespace default --set "developer=$CI_NAMESPACE")
case "$DEPLOY_ENV" in
dev) ;;
prod)
	args+=(
		--set "environment=$DEPLOY_ENV"
		--set bootstrap.enabled=false
		--set webIngressFromVpc.enabled=true
		--set webIngressFromVpc.vpcCidr=10.1.0.0/16
	)
	;;
*)
	echo "::error::DEPLOY_ENV must be dev or prod, got '$DEPLOY_ENV'"
	exit 1
	;;
esac

helm upgrade --install "$CI_NAMESPACE" infra/k8s/charts/er-dev-namespace \
	"${args[@]}" --wait
