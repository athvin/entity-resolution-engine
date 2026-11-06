#!/usr/bin/env bash
# Read the LIVE blue/green colour for the deploy (deploy.yml).
#
# NEVER guessed. The Service selector is the single source of truth for which
# colour is serving — the same field the cutover moves — so this cannot drift
# from reality the way a stored value or a convention would. `make dev-resume`
# reads it the same way (D-BLUEGREEN).
#
# Absent selector == first install: there is no live colour, so there is
# nothing to cut over FROM and the deploy takes the simple path.
#
# Inputs (env): CI_NAMESPACE.
# Outputs ($GITHUB_OUTPUT): first, live, target, live_api, live_web.
set -euo pipefail

live="$(kubectl -n "$CI_NAMESPACE" get svc er-web \
	-o jsonpath='{.spec.selector.er\.dupezero\.com/color}' 2>/dev/null || true)"
if [ -z "$live" ]; then
	echo "no live colour (first install) — deploying blue directly"
	echo "first=true" >> "$GITHUB_OUTPUT"
	echo "live=blue" >> "$GITHUB_OUTPUT"
	echo "target=blue" >> "$GITHUB_OUTPUT"
	exit 0
fi
case "$live" in
blue) target=green ;;
green) target=blue ;;
*)
	echo "::error::svc/er-web selects unknown colour '$live'"
	exit 1
	;;
esac
# The digests the LIVE colour is already serving, so the upgrade that deploys
# the target does not re-render the live Deployment and restart the pod taking
# traffic. `image: reg/repo@sha256:…` -> the digest only.
live_api="$(kubectl -n "$CI_NAMESPACE" get deploy "web-api-$live" \
	-o jsonpath='{.spec.template.spec.containers[?(@.name=="er-server")].image}' | sed 's/.*@//')"
live_web="$(kubectl -n "$CI_NAMESPACE" get deploy "web-api-$live" \
	-o jsonpath='{.spec.template.spec.containers[?(@.name=="er-web")].image}' | sed 's/.*@//')"
echo "live=$live target=$target"
echo "live api=$live_api web=$live_web"
{
	echo "first=false"
	echo "live=$live"
	echo "target=$target"
	echo "live_api=$live_api"
	echo "live_web=$live_web"
} >> "$GITHUB_OUTPUT"
