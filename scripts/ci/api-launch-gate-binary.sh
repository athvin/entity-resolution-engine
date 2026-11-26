#!/usr/bin/env bash
# Put the release's own `er-server` on the runner, for the API launch gate
# (#658). The deploy job has no Rust toolchain, and the comparison must be the
# one the code ships (`er-server api-check`), so the binary is copied out of
# the er-api image the deploy is about to roll — by digest, the exact release.
#
# Inputs (env): API_DIGEST (sha256:…), AWS_REGION. Exports API_CHECK to
# $GITHUB_ENV: the command api-launch-gate.sh calls.
set -euo pipefail

: "${API_DIGEST:?API_DIGEST is required}"
registry="797781631727.dkr.ecr.${AWS_REGION:-us-east-2}.amazonaws.com"
aws ecr get-login-password --region "${AWS_REGION:-us-east-2}" \
    | docker login --username AWS --password-stdin "$registry" >/dev/null
container="$(docker create "$registry/er-api@$API_DIGEST")"
trap 'docker rm "$container" >/dev/null' EXIT
dest="$RUNNER_TEMP/er-server"
docker cp "$container:/usr/local/bin/er-server" "$dest"
chmod +x "$dest"
echo "API_CHECK=$dest api-check" >>"$GITHUB_ENV"
echo "api launch gate: checker is er-server from er-api@$API_DIGEST"
