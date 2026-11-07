#!/usr/bin/env bash
# Run the blue/green zero-downtime acceptance criterion against a live
# namespace, and MEASURE it rather than eyeballing it (#656 criteria 1 and 2).
#
# WHY A SCRIPT AND NOT A RUNBOOK. The blue/green stack shipped across 14 pull
# requests, each verified by content on `main`, and the end-to-end criterion the
# plan specified was never run — because running it by hand is a dozen commands
# with a timing-sensitive loop in the middle, which is exactly the shape of
# thing that stays un-run. The claim "zero-downtime cutover" is a measurement,
# so it needs something that can be re-run after any change to the deploy path.
#
# WHAT IT ASSERTS. One request loop runs for the whole window, against the LIVE
# Services (not the preview ones), covering a document route and an API route.
# Zero non-2xx on either is the criterion. A 404 for a stale
# `/_next/static/<old-build-id>/…` asset is EXPECTED and deliberately not
# requested here: the App Router recovers with a hard navigation, the real fix
# is per-target-group ALB stickiness which D-BLUEGREEN defers, and counting it
# would fail a cutover that worked. See the issue's "known residual risk".
#
# THE CONCURRENCY GUARD IS LOAD-BEARING. `deploy.yml` performs its own
# three-phase cutover against this namespace on every qualifying merge, so a
# merge landing mid-window would move the colour under the loop and the
# measurement would be of two actors, not one. The Helm revision is recorded
# before and after and must only have advanced by this script's own upgrades.
# A mismatch aborts rather than reporting a number nobody can trust.
#
# IT DOES NOT RETIRE THE OLD COLOUR. `deploy-retire-colour.sh` scales the old
# colour to zero after a verified cutover; this script leaves both colours up so
# the rollback it measures is the cheap one (a selector move). That is the state
# the issue's criterion 2 describes, and it is NOT the steady state — see the
# note this script prints at the end.
#
# Inputs (env):
#   CI_NAMESPACE  namespace to exercise (default `ci`)
#   LOOP_INTERVAL seconds between request rounds (default 0.25)
#   SETTLE        seconds to keep looping after the flip returns (default 15)
# Run:  bash scripts/ci/bluegreen-acceptance.sh
# Exit: 0 criterion met · 1 criterion failed · 2 could not run
set -uo pipefail

cd "$(cd "$(dirname "$0")/../.." && pwd)" || exit 2

NS="${CI_NAMESPACE:-ci}"
INTERVAL="${LOOP_INTERVAL:-0.25}"
SETTLE="${SETTLE:-15}"
CHART=infra/k8s/charts/er-platform
RELEASE=er-platform

say() { printf '\n── %s\n' "$1"; }
die() {
	printf 'bluegreen-acceptance: %s\n' "$1" >&2
	exit 2
}

command -v kubectl >/dev/null || die "kubectl is not on PATH"
command -v helm >/dev/null || die "helm is not on PATH"
kubectl -n "$NS" get svc er-web >/dev/null 2>&1 || die "no svc/er-web in namespace $NS"

colour_of() {
	kubectl -n "$NS" get "svc/$1" \
		-o jsonpath='{.spec.selector.er\.dupezero\.com/color}' 2>/dev/null
}
revision_of() {
	helm list -n "$NS" -o json 2>/dev/null |
		python3 -c 'import json,sys; print(next(r["revision"] for r in json.load(sys.stdin) if r["name"]=="'"$RELEASE"'"))'
}

LIVE="$(colour_of er-web)"
[ -n "$LIVE" ] || die "svc/er-web has no colour selector; is this the blue/green chart?"
case "$LIVE" in
blue) TARGET=green ;;
green) TARGET=blue ;;
*) die "unexpected live colour '$LIVE'" ;;
esac
REV0="$(revision_of)"
[ -n "$REV0" ] || die "could not read the $RELEASE helm revision"

say "live=$LIVE target=$TARGET  helm revision=$REV0  namespace=$NS"

# The measurement is only meaningful if the two colours run the SAME images:
# otherwise a failed request could be the new build rather than the cutover.
vals="$(helm get values "$RELEASE" -n "$NS" -o json 2>/dev/null)" || die "could not read helm values"
same="$(printf '%s' "$vals" | python3 -c '
import json,sys
v=json.load(sys.stdin)["color"]
b,g=v["blue"],v["green"]
print("same" if (b["api"]["digest"],b["web"]["digest"])==(g["api"]["digest"],g["web"]["digest"]) else "differ")')"
printf 'image digests across colours: %s\n' "$same"
[ "$same" = "same" ] || printf 'WARNING: colours run different images — a failure may be the build, not the cutover\n' >&2

# --- bring the inactive colour up ------------------------------------------
say "scaling $TARGET up (the steady state retires it, so this is phase 1)"
helm upgrade "$RELEASE" "$CHART" --namespace "$NS" --reuse-values \
	--set "color.$TARGET.replicas=1" --atomic --wait --timeout 10m >/dev/null \
	|| die "could not scale $TARGET up"
kubectl -n "$NS" rollout status "deploy/web-api-$TARGET" --timeout=5m >/dev/null \
	|| die "$TARGET did not become ready"
printf '%s is ready\n' "$TARGET"

# --- smoke the inactive colour through its PREVIEW service -----------------
# Through preview, because the live Services still point at $LIVE. If the new
# colour cannot serve before the flip, the flip must not happen.
say "smoking $TARGET through svc/er-web-preview"
# Port 3000, not 80: svc/er-web exposes the container port directly. Probing 80
# returns 000 for every request — which looks exactly like an outage and cost a
# diagnosis pass the first time this was written.
kubectl -n "$NS" port-forward svc/er-web-preview 18081:3000 >/dev/null 2>&1 &
PF_PREVIEW=$!
trap 'kill $PF_PREVIEW 2>/dev/null' EXIT
sleep 4
pre="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 http://127.0.0.1:18081/login || echo 000)"
kill $PF_PREVIEW 2>/dev/null
printf 'preview /login -> %s\n' "$pre"
case "$pre" in
2* | 3*) ;;
*) die "$TARGET does not serve through preview (got $pre); refusing to cut over" ;;
esac

# --- the request loop, against the LIVE services ---------------------------
LOG="$(mktemp)"
say "starting the request loop against svc/er-web + svc/er-api"
# BOTH live Services, because the issue's criterion distinguishes them: a
# non-2xx on a document OR a `/v1` request is a failure. er-api's own liveness
# route is the unauthenticated stand-in for `/v1`, which needs a bearer token.
kubectl -n "$NS" port-forward svc/er-web 18080:3000 >/dev/null 2>&1 &
PF_WEB=$!
kubectl -n "$NS" port-forward svc/er-api 18082:8000 >/dev/null 2>&1 &
PF_API=$!
trap 'kill $PF_WEB $PF_API 2>/dev/null; rm -f "$LOG"' EXIT
sleep 5

# `date +%3N` is GNU-only. On macOS it emits a literal `.3N` and EXITS 0, so a
# `|| fallback` never fires and the log fills with unusable timestamps — which
# is how a broken clock reads as a working one. Second resolution is enough:
# the verdict is a count of non-2xx, not sub-second latency.
(
	while [ -f "$LOG" ]; do
		code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:18080/login || echo 000)"
		printf '%s\tweb/login\t%s\n' "$(date -u +%H:%M:%S)" "$code" >>"$LOG"
		code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:18082/healthz || echo 000)"
		printf '%s\tapi/healthz\t%s\n' "$(date -u +%H:%M:%S)" "$code" >>"$LOG"
		sleep "$INTERVAL"
	done
) &
LOOP_PID=$!

sleep 3
before_n=$(wc -l <"$LOG" | tr -d ' ')
printf 'loop is running (%s requests logged before the flip)\n' "$before_n"

# --- the cutover: one field on two Services --------------------------------
say "cutting over $LIVE -> $TARGET"
flip_start=$(date -u +%s)
helm upgrade "$RELEASE" "$CHART" --namespace "$NS" --reuse-values \
	--set "color.active=$TARGET" --atomic --wait --timeout 10m >/dev/null
flip_rc=$?
flip_end=$(date -u +%s)
printf 'helm upgrade returned %s after %ss\n' "$flip_rc" "$((flip_end - flip_start))"

printf 'letting the loop run %ss past the flip\n' "$SETTLE"
sleep "$SETTLE"
kill $LOOP_PID 2>/dev/null
wait $LOOP_PID 2>/dev/null
kill $PF_WEB $PF_API 2>/dev/null

now="$(colour_of er-web)"
printf 'svc/er-web now selects: %s\n' "$now"

# --- the concurrency guard -------------------------------------------------
REV1="$(revision_of)"
printf 'helm revision: %s -> %s (expected +2 from this script)\n' "$REV0" "$REV1"
if [ "$REV1" != "$((REV0 + 2))" ]; then
	printf 'bluegreen-acceptance: ANOTHER ACTOR CHANGED THE RELEASE mid-window.\n' >&2
	printf 'deploy.yml performs its own cutover on a qualifying merge, so this\n' >&2
	printf 'measurement is of two actors and must not be reported. Re-run when quiet.\n' >&2
	exit 2
fi

# --- the verdict -----------------------------------------------------------
say "results"
total=$(wc -l <"$LOG" | tr -d ' ')
bad=$(awk -F'\t' '$3 !~ /^2/ && $3 !~ /^3/' "$LOG")
badn=$(printf '%s' "$bad" | grep -c . || true)
printf 'requests: %s   non-2xx/3xx: %s\n' "$total" "$badn"
printf 'status distribution:\n'
awk -F'\t' '{print "  "$2" "$3}' "$LOG" | sort | uniq -c | sort -rn

if [ "$badn" -gt 0 ]; then
	printf '\nthe failures:\n%s\n' "$bad"
fi

printf '\nNOTE: both colours are still running, so a rollback right now is one\n'
printf 'field moving back and takes seconds:\n'
printf '  helm upgrade %s %s -n %s --reuse-values --set color.active=%s\n' \
	"$RELEASE" "$CHART" "$NS" "$LIVE"
printf 'That is NOT the steady state. deploy-retire-colour.sh scales the old\n'
printf 'colour to zero after a verified cutover, and a rollback after that needs\n'
printf 'the replica count back too — so it costs a pod start, not a selector move.\n'

[ "$now" = "$TARGET" ] || {
	printf '\nFAILED: the cutover did not take\n' >&2
	exit 1
}
[ "$badn" -eq 0 ] || {
	printf '\nFAILED: %s non-2xx responses during the cutover window\n' "$badn" >&2
	exit 1
}
printf '\nPASSED: cutover %s -> %s with %s requests and zero non-2xx\n' "$LIVE" "$TARGET" "$total"
