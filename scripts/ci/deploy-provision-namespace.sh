#!/usr/bin/env bash
# Provision the deploy namespace's EFS access point and secrets, and publish
# the ids the chart needs — BY ENVIRONMENT, because the two environments
# disagree about who owns these objects.
#
# dev (DEPLOY_ENV=dev, the default): `scripts/dev_env.sh up` — the SAME script
# `make dev-up` runs, so CI and a human provision a namespace identically
# rather than by two drifting paths. It is idempotent: it finds an existing
# access point and existing secrets.
#
# prod (DEPLOY_ENV=prod): DISCOVER, never create. Prod's storage and secrets
# are Terraform's (envs/prod/efs.tf owns the /app access point;
# envs/prod/secrets.tf owns the two §9 entries — the one-namespace argument
# both files make). This step finds the file system by its Name tag and the
# access point by its root path — the same query dev_env.sh runs — and
# VERIFIES the two secrets are readable. Anything missing is a hard fail here,
# in seconds: a release installed without its access point hangs its pods at
# mount time, and one without its secrets hangs the ExternalSecret sync, both
# surfacing as a rollout timeout ten minutes later with the cause nowhere in
# it.
#
# prod additionally resolves the edge target group's ARN BY NAME. Committed,
# it would bind to nothing while reading as configured if the group were ever
# recreated; discovered, the binding follows. Empty only if the edge is not
# built yet — the chart renders no TargetGroupBinding then and the deploy is
# still valid.
#
# `eval` is how dev_env.sh's output crosses into this shell — it prints
# `export` assignments rather than bare ids, because there are two of them and
# a human sources it. That is also why the ids are re-emitted to $GITHUB_OUTPUT
# here: an `export` dies with the step, and later steps read them as
# `steps.env.outputs.fs` / `.ap` (and `.tg` for prod).
#
# Inputs (env): CI_NAMESPACE; DEPLOY_ENV (dev|prod, default dev).
# Outputs ($GITHUB_OUTPUT): fs, ap; prod also tg.
# Run: bash scripts/ci/deploy-provision-namespace.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

: "${CI_NAMESPACE:?CI_NAMESPACE must be set}"
DEPLOY_ENV="${DEPLOY_ENV:-dev}"

case "$DEPLOY_ENV" in
dev)
	eval "$(bash scripts/dev_env.sh up "$CI_NAMESPACE")"

	: "${ER_EFS_FS_ID:?dev_env.sh up did not export ER_EFS_FS_ID}"
	: "${ER_EFS_AP_ID:?dev_env.sh up did not export ER_EFS_AP_ID}"

	echo "fs=$ER_EFS_FS_ID" >>"$GITHUB_OUTPUT"
	echo "ap=$ER_EFS_AP_ID" >>"$GITHUB_OUTPUT"
	;;
prod)
	fs="$(aws efs describe-file-systems \
		--query "FileSystems[?Name=='er-prod-config'].FileSystemId" --output text)"
	[ -n "$fs" ] && [ "$fs" != "None" ] || {
		echo "::error::no EFS file system named er-prod-config — envs/prod/efs.tf has not been applied"
		exit 1
	}
	ap="$(aws efs describe-access-points --file-system-id "$fs" \
		--query "AccessPoints[?RootDirectory.Path=='/$CI_NAMESPACE'].AccessPointId" --output text)"
	[ -n "$ap" ] && [ "$ap" != "None" ] || {
		echo "::error::no EFS access point at /$CI_NAMESPACE on $fs — envs/prod/efs.tf owns it; a missing access point means the prod Terraform has not applied"
		exit 1
	}
	for secret in erserver erweb; do
		aws secretsmanager describe-secret \
			--secret-id "er/prod/$CI_NAMESPACE/$secret" >/dev/null || {
			echo "::error::er/prod/$CI_NAMESPACE/$secret unreadable — envs/prod/secrets.tf owns it"
			exit 1
		}
	done
	tg="$(aws elbv2 describe-target-groups --names er-prod-web \
		--query 'TargetGroups[0].TargetGroupArn' --output text 2>/dev/null || true)"
	if [ -z "$tg" ] || [ "$tg" = "None" ]; then
		echo "::notice::no target group named er-prod-web — deploying with no TargetGroupBinding (the edge will 503 until envs/prod/edge.tf applies)"
		tg=""
	fi
	echo "fs=$fs ap=$ap tg=${tg:-none}"
	{
		echo "fs=$fs"
		echo "ap=$ap"
		echo "tg=$tg"
	} >>"$GITHUB_OUTPUT"
	;;
*)
	echo "::error::DEPLOY_ENV must be dev or prod, got '$DEPLOY_ENV'"
	exit 1
	;;
esac
