#!/usr/bin/env bash
# Phase 1 of the blue/green cutover: deploy the INACTIVE colour (deploy.yml).
#
# Traffic does NOT move here: `color.active` stays on the live colour, and the
# new digests go into the other one. THE SCHEMA GATE IS THE HOOK, not a step:
# `er-schema-migrate` (weight -5) runs `er-server migrate --expect-additive`,
# which plans first and refuses — writing nothing — if the release is
# breaking. Helm runs hooks before any release object is applied, so a
# breaking release fails with the live colour untouched.
#
# `images.api/web.digest` is what the HOOK migrates with: the colour blocks
# say what each Deployment runs, while `images.*` means "the release this
# upgrade is about".
#
# Inputs (env): CI_NAMESPACE, LIVE, TARGET, FIRST, LIVE_API, LIVE_WEB,
# NEW_API, NEW_WEB, FS, AP.
set -euo pipefail

args=(
	--namespace "$CI_NAMESPACE" --set "developer=$CI_NAMESPACE"
	--set "images.api.digest=$NEW_API"
	--set "images.web.digest=$NEW_WEB"
	--set "efs.fileSystemId=$FS"
	--set "efs.accessPointId=$AP"
	--set "color.active=$LIVE"
	--set "color.$TARGET.replicas=1"
	--set "color.$TARGET.api.digest=$NEW_API"
	--set "color.$TARGET.web.digest=$NEW_WEB"
)
if [ "$FIRST" != "true" ]; then
	# Pin the live colour to what it is ALREADY serving. Without this the
	# live Deployment re-renders to the new digest and restarts the pod
	# taking traffic, which is the outage this exists to remove.
	args+=(
		--set "color.$LIVE.replicas=1"
		--set "color.$LIVE.api.digest=$LIVE_API"
		--set "color.$LIVE.web.digest=$LIVE_WEB"
	)
fi
helm upgrade --install er-platform infra/k8s/charts/er-platform \
	"${args[@]}" --atomic --wait --timeout 10m
kubectl -n "$CI_NAMESPACE" rollout status "deploy/web-api-$TARGET" --timeout=5m
