#!/usr/bin/env bash
# Self-test for api-launch-gate.sh (#658). Pure bash: a stub `aws` and a stub
# checker drive every decision, so this needs no AWS, no image and no cargo.
#
# A gate breaks by noticing nothing, which reads exactly like a clean release,
# so every REFUSE branch is asserted here, not only the passes.
set -uo pipefail

cd "$(dirname "$0")/../.."
gate=scripts/ci/api-launch-gate.sh
pass=0
fail=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# The stub aws: behaviour chosen by STUB_GET (ok|missing|denied|junk) and
# STUB_PUT (ok|exists|denied); every call is logged to $tmp/calls.
cat >"$tmp/aws" <<'STUB'
#!/usr/bin/env bash
echo "$*" >>"$STUB_DIR/calls"
case "$2" in
  get-object)
    out="${@: -1}"
    case "$STUB_GET" in
      ok) echo '{"openapi":"3.1.0"}' >"$out" ;;
      junk) echo '<html>' >"$out" ;;
      missing) echo "An error occurred (NoSuchKey) when calling the GetObject operation" >&2; exit 254 ;;
      denied) echo "An error occurred (AccessDenied) when calling the GetObject operation" >&2; exit 254 ;;
    esac ;;
  put-object)
    case "$STUB_PUT" in
      ok) ;;
      exists) if [[ "$*" == *if-none-match* ]]; then echo "An error occurred (PreconditionFailed)" >&2; exit 254; fi ;;
      denied) echo "An error occurred (AccessDenied)" >&2; exit 254 ;;
    esac ;;
esac
STUB
chmod +x "$tmp/aws"
# The stub checker exits with STUB_CHECK.
printf '#!/usr/bin/env bash\nexit "$STUB_CHECK"\n' >"$tmp/check"
chmod +x "$tmp/check"
echo '{"openapi":"3.1.0","paths":{}}' >"$tmp/candidate.json"

run() {
    : >"$tmp/calls"
    STUB_DIR="$tmp" AWS="$tmp/aws" API_CHECK="$tmp/check" \
        API_CONTRACT_BUCKET=b DEPLOY_SHA=abc123 CANDIDATE="$tmp/candidate.json" \
        bash "$gate" "$@" >"$tmp/out" 2>&1
}

expect() {
    local want="$1" label="$2"
    shift 2
    local got=0
    run "$@" || got=$?
    if { [ "$want" = pass ] && [ "$got" -eq 0 ]; } || { [ "$want" = refuse ] && [ "$got" -ne 0 ]; }; then
        echo "  PASS  $label"
        pass=$((pass + 1))
    else
        echo "  FAIL  $label (exit $got)"
        sed 's/^/        /' "$tmp/out"
        fail=$((fail + 1))
    fi
}

echo "api-launch-gate check:"
STUB_GET=ok STUB_CHECK=0 expect pass "a compatible release passes" check
STUB_GET=ok STUB_CHECK=2 expect refuse "a breaking release is refused" check
STUB_GET=ok STUB_CHECK=2 API_BREAK_OK="v2 cutover" expect pass "a breaking release passes only with an explicit reason" check
grep -q "v2 cutover" "$tmp/out" && { echo "  PASS  the override's reason is printed"; pass=$((pass + 1)); } || { echo "  FAIL  the override's reason is not printed"; fail=$((fail + 1)); }
STUB_GET=missing STUB_CHECK=2 expect pass "no deployed contract yet is first publication" check
STUB_GET=denied STUB_CHECK=0 expect refuse "an unreadable baseline refuses, even if the checker would pass" check
STUB_GET=ok STUB_CHECK=64 expect refuse "a checker that cannot read its input refuses" check
STUB_GET=ok STUB_CHECK=101 expect refuse "a checker that crashes refuses" check
: >"$tmp/candidate.json"
STUB_GET=ok STUB_CHECK=0 expect refuse "an empty candidate refuses" check
echo '{"openapi":"3.1.0","paths":{}}' >"$tmp/candidate.json"

echo "api-launch-gate publish:"
STUB_PUT=ok expect pass "publish writes the release and the pointer" publish
grep -q "releases/abc123/openapi.public.json.*--if-none-match" "$tmp/calls" && grep -q "deployed/openapi.public.json" "$tmp/calls" && grep -q "deployed/release.json" "$tmp/calls" \
    && { echo "  PASS  the release is written once and the pointer follows"; pass=$((pass + 1)); } \
    || { echo "  FAIL  publish did not write all three objects"; cat "$tmp/calls"; fail=$((fail + 1)); }
STUB_PUT=exists expect pass "a redeploy of a published sha keeps the release" publish
STUB_PUT=denied expect refuse "a publish that cannot write fails" publish
expect refuse "no verb is a usage error"

echo "api-launch-gate-test: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
