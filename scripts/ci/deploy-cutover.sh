#!/usr/bin/env bash
# Phase 2 of the blue/green cutover: move traffic (deploy.yml).
#
# The whole cutover is one field on two Service objects. The old colour keeps
# running, so a rollback is the same field moving back — the summary this
# writes prints the command while that is still true.
#
# The optional inputs are the same seam deploy-inactive-colour.sh documents,
# and they are LOAD-BEARING HERE rather than repeated for symmetry: this step
# re-renders the release, so a value the inactive phase set and this one omits
# is REVERTED to the chart default at cutover — for prod that deletes the
# TargetGroupBinding at the exact moment traffic moves to it.
#
# Inputs (env): CI_NAMESPACE, LIVE, TARGET, LIVE_API, LIVE_WEB,
# NEW_API, NEW_WEB, FS, AP; optional PLATFORM_VALUES, TARGET_GROUP_ARN.
set -euo pipefail

args=(
	--namespace "$CI_NAMESPACE" --set "developer=$CI_NAMESPACE" \
	--set "images.api.digest=$NEW_API" --set "images.web.digest=$NEW_WEB" \
	--set "efs.fileSystemId=$FS" --set "efs.accessPointId=$AP" \
	--set "color.active=$TARGET" \
	--set "color.$TARGET.replicas=1" \
	--set "color.$TARGET.api.digest=$NEW_API" \
	--set "color.$TARGET.web.digest=$NEW_WEB" \
	--set "color.$LIVE.replicas=1" \
	--set "color.$LIVE.api.digest=$LIVE_API" \
	--set "color.$LIVE.web.digest=$LIVE_WEB" \
)
if [ -n "${PLATFORM_VALUES:-}" ]; then
	args+=(-f "$PLATFORM_VALUES")
fi
if [ -n "${TARGET_GROUP_ARN:-}" ]; then
	args+=(--set "edge.targetGroupArn=$TARGET_GROUP_ARN")
fi
helm upgrade er-platform infra/k8s/charts/er-platform \
	"${args[@]}" --atomic --wait --timeout 10m
now="$(kubectl -n "$CI_NAMESPACE" get svc er-web \
	-o jsonpath='{.spec.selector.er\.dupezero\.com/color}')"
[ "$now" = "$TARGET" ] || {
	echo "::error::cutover did not take: svc selects '$now'"
	exit 1
}
echo "traffic is on $TARGET"
{
	echo "### Cut over to \`$TARGET\`"
	echo
	echo "The \`$LIVE\` colour is still running. To roll back:"
	echo
	echo '```'
	echo "helm upgrade er-platform infra/k8s/charts/er-platform \\"
	echo "  --namespace $CI_NAMESPACE --reuse-values --set color.active=$LIVE"
	echo '```'
} >> "$GITHUB_STEP_SUMMARY"
