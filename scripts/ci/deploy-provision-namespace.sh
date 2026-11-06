#!/usr/bin/env bash
# Provision the deploy namespace's EFS access point and secrets, and publish
# the two ids the chart needs.
#
# `scripts/dev_env.sh up` is the SAME script `make dev-up` runs, so CI and a
# human provision a namespace identically rather than by two drifting paths.
# It is idempotent: it finds an existing access point and existing secrets.
#
# `eval` is how its output crosses into this shell — it prints `export`
# assignments rather than bare ids, because there are two of them and a human
# sources it. That is also why the ids are re-emitted to $GITHUB_OUTPUT here:
# an `export` dies with the step, and later steps read them as
# `steps.env.outputs.fs` / `.ap`.
#
# Inputs (env): CI_NAMESPACE. Outputs ($GITHUB_OUTPUT): fs, ap.
# Run: bash scripts/ci/deploy-provision-namespace.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

: "${CI_NAMESPACE:?CI_NAMESPACE must be set}"

eval "$(bash scripts/dev_env.sh up "$CI_NAMESPACE")"

: "${ER_EFS_FS_ID:?dev_env.sh up did not export ER_EFS_FS_ID}"
: "${ER_EFS_AP_ID:?dev_env.sh up did not export ER_EFS_AP_ID}"

echo "fs=$ER_EFS_FS_ID" >> "$GITHUB_OUTPUT"
echo "ap=$ER_EFS_AP_ID" >> "$GITHUB_OUTPUT"
