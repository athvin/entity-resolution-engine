#!/usr/bin/env bash
# Phase 2 of the blue/green cutover: move traffic (deploy.yml).
#
# The whole cutover is one field on two Service objects. The old colour keeps
# running, so a rollback is the same field moving back — the summary this
# writes prints the command while that is still true.
#
# Inputs (env): CI_NAMESPACE, LIVE, TARGET, LIVE_API, LIVE_WEB,
# NEW_API, NEW_WEB, FS, AP.
set -euo pipefail

helm upgrade er-platform infra/k8s/charts/er-platform \
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
	--atomic --wait --timeout 10m
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
