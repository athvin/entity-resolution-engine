#!/usr/bin/env bash
# Runs INSIDE the acceptance pod under its actual tenant ServiceAccount.
# Only the reserved synthetic prefixes are accepted. No customer objects.
set -euo pipefail
: "${PROBE_BUCKET:?}"
: "${OWN_PREFIX:?}"
: "${OTHER_PREFIX:?}"
case "$OWN_PREFIX:$OTHER_PREFIX" in
    _isolation/probe-a/:_isolation/probe-b/|_isolation/probe-b/:_isolation/probe-a/) ;;
    *) echo 'refusing non-fixture prefixes' >&2; exit 2 ;;
esac
probe_cli="${AWS_CLI:-aws}"
probe_key="${OWN_PREFIX}boundary-marker"
probe_tmp="$(mktemp -d)"
marker=tenant-isolation-positive-control
printf '%s' "$marker" > "$probe_tmp/marker"
cleanup() {
    local result=$?
    "$probe_cli" s3api delete-object --bucket "$PROBE_BUCKET" --key "$probe_key" >/dev/null || result=1
    rm -rf "$probe_tmp"
    exit "$result"
}
# The tiny marker's noncurrent version follows the lake's 30-day lifecycle.
# The runner deliberately has no DeleteObjectVersion capability.
trap cleanup EXIT

expect_denied() {
    local label="$1"
    shift
    if "$probe_cli" "$@" >"$probe_tmp/result" 2>"$probe_tmp/error"; then
        echo "FAIL: $label was allowed" >&2
        exit 1
    fi
    if ! grep -Fq '(AccessDenied)' "$probe_tmp/error"; then
        echo "FAIL: $label did not return AccessDenied" >&2
        exit 1
    fi
    echo "PASS: $label denied"
}

"$probe_cli" s3api put-object --bucket "$PROBE_BUCKET" --key "$probe_key" --body "$probe_tmp/marker" >/dev/null
"$probe_cli" s3api get-object --bucket "$PROBE_BUCKET" --key "$probe_key" "$probe_tmp/read" >/dev/null
[ "$(cat "$probe_tmp/read")" = "$marker" ]
"$probe_cli" s3api list-objects-v2 --bucket "$PROBE_BUCKET" --prefix "$OWN_PREFIX" --max-keys 1 >/dev/null
echo 'PASS: own-prefix put/get/list'
expect_denied cross-prefix-put s3api put-object --bucket "$PROBE_BUCKET" --key "${OTHER_PREFIX}boundary-marker" --body "$probe_tmp/marker"
expect_denied cross-prefix-get s3api get-object --bucket "$PROBE_BUCKET" --key "${OTHER_PREFIX}boundary-marker" "$probe_tmp/other"
expect_denied cross-prefix-list s3api list-objects-v2 --bucket "$PROBE_BUCKET" --prefix "$OTHER_PREFIX" --max-keys 1
expect_denied cross-prefix-delete s3api delete-object --bucket "$PROBE_BUCKET" --key "${OTHER_PREFIX}boundary-marker"
expect_denied bucket-root-list s3api list-objects-v2 --bucket "$PROBE_BUCKET" --max-keys 1
echo 'PASS: tenant S3 isolation'
