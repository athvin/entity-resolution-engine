#!/usr/bin/env bash
# Phase 3 of the cutover: scale the colour that was live down to zero.
#
# Deliberately NOT --atomic-coupled to the cutover. It runs only after the
# cutover has been verified, so a failure here leaves traffic correctly on the
# new colour with one surplus pod — the harmless direction.
#
# SCALED TO ZERO RATHER THAN DELETED: two serving pods is double the
# steady-state cost of a namespace, and keeping the Deployment means the next
# deploy scales it up with its image already cached. A rollback after this
# point needs the replica count back as well as the selector, which is why
# scripts/ci/deploy-summary.sh prints that command while the pod is still
# there.
#
# Inputs (env): CI_NAMESPACE, LIVE (the colour being retired).
# Run: bash scripts/ci/deploy-retire-colour.sh
set -euo pipefail

: "${CI_NAMESPACE:?CI_NAMESPACE must be set}"
: "${LIVE:?LIVE must name the colour being retired}"

kubectl -n "$CI_NAMESPACE" scale "deploy/web-api-$LIVE" --replicas=0
echo "retired $LIVE"
