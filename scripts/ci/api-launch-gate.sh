#!/usr/bin/env bash
# The API launch gate (#658): refuse a production release whose public API
# contract breaks what production is serving, and record what was served.
#
#   api-launch-gate.sh check    # before the inactive colour is deployed
#   api-launch-gate.sh publish  # after cutover succeeded
#
# The baseline is the DEPLOYED contract (s3://$API_CONTRACT_BUCKET/deployed/),
# not origin/main: main moves without a deploy, and only the deployed document
# is what an external consumer is integrated against.
#
# check — the decisions, each one named in the log so a reader can tell which
# path a release took:
#   baseline absent (NoSuchKey)   -> first publication: pass, no comparison
#   baseline unreadable (any other read failure, or not JSON)
#                                 -> REFUSE. An unreadable baseline must never
#                                    silently approve a customer-facing release
#   candidate compatible          -> pass
#   candidate breaking, no reason -> REFUSE
#   candidate breaking, API_BREAK_OK set (a deliberate version change or a
#   rollback, by workflow_dispatch input)
#                                 -> pass, with the reason printed
#   the checker fails to run      -> REFUSE
#
# publish — releases/<sha>/openapi.public.json written once (If-None-Match: *;
# an existing object for the same sha is a redeploy and is left alone), then the
# deployed/ pointer and its release.json.
#
# Inputs (env): API_CONTRACT_BUCKET, DEPLOY_SHA, CANDIDATE (path to the
# release's openapi.public.json), API_BREAK_OK (optional reason), API_CHECK (the
# checker command, called as `$API_CHECK <base> <candidate> --strict`), and AWS
# (defaults to `aws`; the self-test substitutes a stub).
set -uo pipefail

: "${API_CONTRACT_BUCKET:?API_CONTRACT_BUCKET is required}"
: "${DEPLOY_SHA:?DEPLOY_SHA is required}"
: "${CANDIDATE:?CANDIDATE is required}"
AWS="${AWS:-aws}"
API_BREAK_OK="${API_BREAK_OK:-}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

check() {
    : "${API_CHECK:?API_CHECK is required for check}"
    if [ ! -s "$CANDIDATE" ]; then
        echo "::error::api launch gate: the candidate contract $CANDIDATE is missing or empty"
        return 1
    fi
    if ! "$AWS" s3api get-object --bucket "$API_CONTRACT_BUCKET" \
        --key deployed/openapi.public.json "$work/base.json" >/dev/null 2>"$work/err"; then
        if grep -q "NoSuchKey" "$work/err"; then
            echo "api launch gate: no deployed contract yet; first publication, no comparison"
            return 0
        fi
        echo "::error::api launch gate: the deployed contract is UNREADABLE, so this release cannot be checked and is refused:"
        cat "$work/err"
        return 1
    fi
    local code=0
    $API_CHECK "$work/base.json" "$CANDIDATE" --strict || code=$?
    case "$code" in
        0)
            echo "api launch gate: compatible with the deployed contract"
            return 0
            ;;
        2)
            if [ -n "$API_BREAK_OK" ]; then
                echo "::warning::api launch gate: BREAKING changes accepted by an explicit override: $API_BREAK_OK"
                return 0
            fi
            echo "::error::api launch gate: this release breaks the deployed public contract (listed above). Re-run the deploy with an api_break_ok reason only for a deliberate version change or a rollback."
            return 1
            ;;
        *)
            echo "::error::api launch gate: the checker failed to run (exit $code); refusing rather than guessing"
            return 1
            ;;
    esac
}

publish() {
    local published_at
    published_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    if ! "$AWS" s3api put-object --bucket "$API_CONTRACT_BUCKET" \
        --key "releases/$DEPLOY_SHA/openapi.public.json" --body "$CANDIDATE" \
        --if-none-match '*' >/dev/null 2>"$work/err"; then
        if grep -q "PreconditionFailed" "$work/err"; then
            echo "api launch gate: release $DEPLOY_SHA was already published; keeping it"
        else
            echo "::error::api launch gate: publishing release $DEPLOY_SHA failed:"
            cat "$work/err"
            return 1
        fi
    fi
    printf '{"sha":"%s","published_at":"%s"}\n' "$DEPLOY_SHA" "$published_at" >"$work/release.json"
    "$AWS" s3api put-object --bucket "$API_CONTRACT_BUCKET" \
        --key deployed/openapi.public.json --body "$CANDIDATE" >/dev/null || return 1
    "$AWS" s3api put-object --bucket "$API_CONTRACT_BUCKET" \
        --key deployed/release.json --body "$work/release.json" >/dev/null || return 1
    echo "api launch gate: production now serves the contract of $DEPLOY_SHA"
}

case "${1:-}" in
    check) check ;;
    publish) publish ;;
    *)
        echo "usage: api-launch-gate.sh check|publish" >&2
        exit 64
        ;;
esac
