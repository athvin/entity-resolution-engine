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
# Two optional inputs carry an environment's extra surface. PLATFORM_VALUES is
# a values FILE layered under every --set here (`--set` always wins over `-f`),
# which is where prod's static values live (values-prod.yaml) so that the three
# helm-driving scripts share one definition rather than three copies that can
# drift. TARGET_GROUP_ARN is prod's one dynamic value, discovered by name in
# deploy-provision-namespace.sh; empty renders no TargetGroupBinding.
#
# Inputs (env): CI_NAMESPACE, LIVE, TARGET, FIRST, LIVE_API, LIVE_WEB,
# NEW_API, NEW_WEB, FS, AP; optional PLATFORM_VALUES, TARGET_GROUP_ARN.
set -euo pipefail

# §3.5's refusal, at the boundary where it cannot be bypassed: a deploy passes
# a DIGEST, and anything else — a tag, a bare repository, an empty string —
# stops the release here rather than pulling something mutable. The resolve
# step only ever emits digests, so a failure here means the inputs were
# hand-fed or the resolver was bypassed, both of which must fail loudly.
for digest in "$NEW_API" "$NEW_WEB"; do
	case "$digest" in
	sha256:*) ;;
	*)
		echo "::error::refusing to deploy '$digest' — §14.2 deploys reference an image by sha256 digest, never a tag"
		exit 1
		;;
	esac
done

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
if [ -n "${PLATFORM_VALUES:-}" ]; then
	args+=(-f "$PLATFORM_VALUES")
fi
if [ -n "${TARGET_GROUP_ARN:-}" ]; then
	args+=(--set "edge.targetGroupArn=$TARGET_GROUP_ARN")
fi
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
