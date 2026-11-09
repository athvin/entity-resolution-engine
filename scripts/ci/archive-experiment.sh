#!/usr/bin/env bash
# Same suite and services in both arms; only compilation placement differs.
set -euo pipefail
case "${EXPERIMENT_MODE:?}" in
build)
    cargo nextest archive -p er-cli --archive-file "$RUNNER_TEMP/er-cli-tests.tar.zst"
    ;;
baseline)
    cargo nextest run -p er-cli --no-fail-fast --partition "hash:${PARTITION:?}"
    ;;
archive)
    cargo nextest run --archive-file "$RUNNER_TEMP/er-cli-tests.tar.zst" \
        --workspace-remap "$PWD" --no-fail-fast --partition "hash:${PARTITION:?}"
    ;;
*) echo 'unknown experiment arm' >&2; exit 2 ;;
esac
