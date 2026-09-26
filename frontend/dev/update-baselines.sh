#!/usr/bin/env bash
# Regenerate the committed visual baselines inside the pinned Playwright Linux
# image (the same OS family CI compares on). The project is copied inside the
# container and installed fresh there — a bind-mounted node_modules would mix
# Linux and macOS native binaries — and only the *-snapshots/ directories are
# copied back out.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FRONTEND_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
IMAGE="mcr.microsoft.com/playwright:v1.63.0-jammy"

docker run --rm -v "${FRONTEND_DIR}:/host" -w /tmp "${IMAGE}" bash -eu -o pipefail -c '
  mkdir -p /tmp/app
  cd /host
  tar --exclude=node_modules --exclude=.next --exclude=test-results \
      --exclude=playwright-report --exclude=dev/.state -cf - . | tar -xf - -C /tmp/app
  cd /tmp/app
  corepack enable
  corepack prepare --activate
  pnpm install --frozen-lockfile
  pnpm build
  pnpm test:e2e --update-snapshots --grep @visual
  cd tests
  find . -type d -name "*-snapshots" | while read -r dir; do
    rm -rf "/host/tests/${dir#./}"
    mkdir -p "/host/tests/$(dirname "${dir#./}")"
    cp -R "$dir" "/host/tests/${dir#./}"
  done
'

echo "baselines updated under frontend/tests/**/*-snapshots/ — review and commit them"
