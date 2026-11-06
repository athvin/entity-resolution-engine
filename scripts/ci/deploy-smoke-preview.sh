#!/usr/bin/env bash
# Smoke-test the inactive colour THROUGH ITS PREVIEW SERVICES.
#
# The preview Services select the inactive colour. Hitting the live ones would
# test what is already in production and pass for the wrong reason — the
# failure that would make this whole blue/green mechanism decorative without
# looking broken. So the hostnames here are `er-api-preview` and
# `er-web-preview`, never the live names.
#
# Readiness already proves er-server reached the control plane and the BFF
# reached er-server (#437 pointed both probes at checks that can fail). This
# adds the one thing a probe cannot: that the answer arrives THROUGH A
# SERVICE, over the network path a real request takes.
#
# Run as a pod in the namespace rather than with `kubectl port-forward`: a
# forward would prove the pod answers, not that the Service routes to it,
# which is the only thing in question here.
#
# The inner script is single-quoted and runs in the curl image's `sh`, so it
# must stay POSIX and must not expand anything from this shell — the two
# `$code` reads belong to that `sh`, not to this one.
#
# Inputs (env): CI_NAMESPACE, TARGET (the colour, for the closing line),
# GITHUB_RUN_ID (names the pod so concurrent runs cannot collide).
# Run: bash scripts/ci/deploy-smoke-preview.sh
set -euo pipefail

: "${CI_NAMESPACE:?CI_NAMESPACE must be set}"
: "${TARGET:?TARGET must name the colour under test}"

kubectl -n "$CI_NAMESPACE" run "smoke-${GITHUB_RUN_ID:-local}" --rm -i --restart=Never \
	--image=curlimages/curl:8.11.1 --quiet -- \
	sh -ec '
	  code=$(curl -sS -o /dev/null -w "%{http_code}" --max-time 10 http://er-api-preview:8000/readyz)
	  echo "er-api-preview /readyz -> $code"
	  [ "$code" = "200" ] || exit 1
	  code=$(curl -sS -o /dev/null -w "%{http_code}" --max-time 10 http://er-web-preview:3000/api/healthz)
	  echo "er-web-preview /api/healthz -> $code"
	  [ "$code" = "200" ] || exit 1
	'
echo "the $TARGET colour serves through its own Services"
