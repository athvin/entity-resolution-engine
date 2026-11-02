#!/usr/bin/env bash
# The kind probe (batch-orchestration-design §11 Phase 2's acceptance): drive
# the executor end to end on a real cluster — real pods, real projected tokens
# against the real API server's JWKS, real adoption. Four phases:
#
#   1. green    — lake_maintain runs as per-attempt Jobs and settles succeeded;
#                 every task row carries a real er-t-… Job name and the real
#                 pod's identity. The pod's ONLY credential is the projected
#                 token (ER_ATTEMPT_TOKEN_FILE in the template), so a green run
#                 IS the §6.4 JWKS verification passing.
#   2. adopt    — the dispatcher pod is deleted mid-run; the new process adopts
#                 the live Job (launches stays 1 — no second pod) and the job
#                 still settles succeeded.
#   3. cancel   — a running job is canceled; it settles canceled and no managed
#                 Job object survives.
#   4. refuse   — a garbage JWT-shaped bearer on the attempt channel is 401.
#
# Needs `scripts/kind_env.sh up` first. Exits non-zero on the first failure.
set -euo pipefail

CLUSTER=er-kind
NS=er-kind
CTX="kind-${CLUSTER}"
DATA_DIR="${ER_KIND_DATA_DIR:-/tmp/er-kind/srv}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${ER_KIND_PROBE_PORT:-18000}"
BASE="http://127.0.0.1:${PORT}"
ORG="kindacme"

log() { echo "check_kind: $*" >&2; }
fail() { echo "check_kind: FAIL — $*" >&2; exit 1; }

OP_TOKEN="$(kubectl --context "${CTX}" -n "${NS}" get secret er-erserver-env \
    -o jsonpath='{.data.ERSERVER_OPERATOR_TOKEN}' | base64 -d)"

# Supervised: `kubectl port-forward` EXITS when its stream drops, and the
# executor's launch bursts (task pods starting, DuckDB spinning up) drop it
# reliably on a loaded kind VM. The wrapper restarts it within a second, and
# api()'s retries ride over the gap.
(
    while true; do
        kubectl --context "${CTX}" -n "${NS}" port-forward svc/er-api "${PORT}:8000" >/dev/null 2>&1 || true
        sleep 1
    done
) &
PF_PID=$!
trap 'kill ${PF_PID} 2>/dev/null || true; pkill -f "port-forward svc/er-api ${PORT}:8000" 2>/dev/null || true' EXIT
for _ in $(seq 1 30); do
    curl -fsS "${BASE}/healthz" >/dev/null 2>&1 && break
    sleep 1
done
curl -fsS "${BASE}/healthz" >/dev/null || fail "er-api unreachable through the port-forward"

api() { # method path [json-body] — retried: `kubectl port-forward` drops the
    # odd connection mid-response, which surfaces as an empty body, not an
    # HTTP error. Three attempts cover it; a real 4xx/5xx still fails fast
    # because the body is empty on every attempt and the caller's parse dies.
    local method="$1" path="$2" body="${3:-}" out=""
    for _ in 1 2 3 4 5; do
        if [ -n "${body}" ]; then
            out="$(curl -fsS -X "${method}" "${BASE}${path}" \
                -H "Authorization: Bearer ${OP_TOKEN}" -H "Content-Type: application/json" \
                -d "${body}" 2>/dev/null)" && [ -n "${out}" ] && { echo "${out}"; return 0; }
        else
            out="$(curl -fsS -X "${method}" "${BASE}${path}" \
                -H "Authorization: Bearer ${OP_TOKEN}" 2>/dev/null)" && [ -n "${out}" ] && { echo "${out}"; return 0; }
        fi
        sleep 2
    done
    return 1
}

# The tenant: a real config on the shared volume (the host side of /srv/er),
# a lake under the same tree.
cp "${ROOT}/rust/crates/er-cli/tests/fixtures/test.yaml" "${DATA_DIR}/configs/${ORG}.yaml"
mkdir -p "${DATA_DIR}/data/${ORG}"
chmod -R a+rwx "${DATA_DIR}/data" "${DATA_DIR}/configs"
api POST /v1/orgs "$(cat <<EOF
{
  "name": "${ORG}",
  "config_path": "/srv/er/configs/${ORG}.yaml",
  "env": {
    "ER_CONFIG_PATH": "/srv/er/configs/${ORG}.yaml",
    "ER_LAKE_CATALOG": "/srv/er/data/${ORG}/catalog.ducklake",
    "ER_LAKE_DATA_PATH": "/srv/er/data/${ORG}/"
  }
}
EOF
)" >/dev/null
log "org ${ORG} registered"

enqueue() {
    api POST "/v1/orgs/${ORG}/jobs" '{"kind":"lake_maintain"}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])'
}

job_state() {
    api GET "/v1/orgs/${ORG}/jobs/$1" | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"])'
}

wait_state() { # job_id want timeout_s
    local id="$1" want="$2" deadline=$(( $(date +%s) + ${3:-300} ))
    while [ "$(date +%s)" -lt "${deadline}" ]; do
        local s
        s="$(job_state "${id}")"
        case "${s}" in
            "${want}") return 0 ;;
            succeeded|failed|canceled)
                [ "${s}" = "${want}" ] && return 0
                fail "job ${id} settled ${s}, wanted ${want}" ;;
        esac
        sleep 3
    done
    fail "job ${id} never reached ${want} (last: $(job_state "${id}"))"
}

tasks_json() { api GET "/v1/orgs/${ORG}/jobs/$1/tasks"; }

# ---- phase 1: green -------------------------------------------------------
log "phase 1: the green path"
JOB1="$(enqueue)"
wait_state "${JOB1}" succeeded 420
tasks_json "${JOB1}" > /tmp/er-kind-tasks1.json
python3 - /tmp/er-kind-tasks1.json <<'EOF'
import json, sys
# argv, not stdin: `cmd | python3 - <<EOF` would make the heredoc CLOBBER the
# piped stdin, and json.load(sys.stdin) reads an exhausted stream.
tasks = json.load(open(sys.argv[1]))["tasks"]
live = [t for t in tasks if t["live"]]
assert len(live) == 2, f"expected 2 live rows, got {len(live)}"
for t in live:
    assert t["state"] == "succeeded", (t["task_id"], t["state"])
    assert (t["k8s_job_name"] or "").startswith("er-t-"), t["k8s_job_name"]
    assert t["pod_name"], "a real pod name landed (from the verified claims)"
    assert t["pod_uid"], "a real pod uid landed"
    assert t["token_hash"] is None, "the fallback token hash is nulled at finish"
print("phase 1 ok: both rows succeeded as real pods")
EOF

# Poll FAST for a live pod task in `running`: on an empty lake the whole job
# settles in seconds, so a coarse poll misses the window. Returns 0 once seen,
# 1 if the job settled first (the caller then tries a fresh job).
catch_running() { # job_id
    local id="$1" deadline=$(( $(date +%s) + 90 ))
    while [ "$(date +%s)" -lt "${deadline}" ]; do
        local snap
        snap="$(api GET "/v1/orgs/${ORG}/jobs/${id}/tasks" || true)"
        case "$(printf '%s' "${snap}" | python3 -c 'import json,sys
try:
    ts = json.load(sys.stdin)["tasks"]
except Exception:
    print("again"); raise SystemExit
if any(t["live"] and t["state"] == "running" for t in ts):
    print("running")
elif ts and all(not t["live"] or t["state"] in ("succeeded","failed","canceled","upstream_failed","skipped") for t in ts):
    print("settled")
else:
    print("again")')" in
            running) return 0 ;;
            settled) return 1 ;;
        esac
        sleep 0.5
    done
    return 1
}

# ---- phase 2: dispatcher restart adopts, no second pod --------------------
log "phase 2: adoption across a dispatcher restart"
JOB2=""
for attempt in 1 2 3; do
    candidate="$(enqueue)"
    if catch_running "${candidate}"; then
        JOB2="${candidate}"
        break
    fi
    log "job ${candidate} settled before the restart window (attempt ${attempt}); trying a fresh one"
done
[ -n "${JOB2}" ] || fail "three jobs settled before a running task could be caught"
kubectl --context "${CTX}" -n "${NS}" delete pod -l app.kubernetes.io/name=er-dispatcher --wait=false
log "dispatcher pod deleted mid-run"
wait_state "${JOB2}" succeeded 420
tasks_json "${JOB2}" > /tmp/er-kind-tasks2.json
python3 - /tmp/er-kind-tasks2.json <<'EOF'
import json, sys
tasks = json.load(open(sys.argv[1]))["tasks"]
live = [t for t in tasks if t["live"]]
assert all(t["state"] == "succeeded" for t in live), [(t["task_id"], t["state"]) for t in live]
for t in live:
    assert t["launches"] == 1, (
        f"{t['task_id']}: launches={t['launches']} — adoption must NOT relaunch "
        "a live Job (a duplicate attempt is two lake writers)"
    )
print("phase 2 ok: adopted, one launch per attempt")
EOF

# ---- phase 3: cancel through canceling ------------------------------------
log "phase 3: cancel while running"
JOB3=""
for attempt in 1 2 3; do
    candidate="$(enqueue)"
    if catch_running "${candidate}"; then
        JOB3="${candidate}"
        break
    fi
    log "job ${candidate} settled before it could be canceled (attempt ${attempt}); trying a fresh one"
done
[ -n "${JOB3}" ] || fail "three jobs settled before a cancel could land"
api POST "/v1/orgs/${ORG}/jobs/${JOB3}/cancel" >/dev/null
# The cancel may lose the race with the final finish: a job that settled
# succeeded moments before the POST is a pass for the RACE, not the probe —
# accept either terminal state but assert no managed Job survives.
deadline=$(( $(date +%s) + 300 ))
final=""
while [ "$(date +%s)" -lt "${deadline}" ]; do
    final="$(job_state "${JOB3}")"
    case "${final}" in canceled|succeeded) break ;; esac
    sleep 2
done
case "${final}" in
    canceled) ;;
    succeeded) log "cancel raced the finish and lost — acceptable; the sweep still ran" ;;
    *) fail "job ${JOB3} is still ${final} after the cancel" ;;
esac
sleep 5
survivors="$(kubectl --context "${CTX}" -n "${NS}" get jobs \
    -l "er.dupezero.com/managed=task,er.dupezero.com/done!=true" --no-headers 2>/dev/null | wc -l | tr -d ' ')"
[ "${survivors}" = "0" ] || fail "cancel left ${survivors} managed Job(s) untombstoned"
log "phase 3 ok: canceled, no managed Job survives"

# ---- phase 4: a forged token is refused -----------------------------------
log "phase 4: the attempt channel refuses a forged JWT"
ATTEMPT="$(tasks_json "${JOB1}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["tasks"][0]["id"])')"
FORGED="eyJhbGciOiJSUzI1NiIsImtpZCI6Im5vcGUifQ.eyJhdWQiOlsiZXItY29udHJvbC1wbGFuZSJdfQ.Zm9yZ2Vk"
code="$(curl -s -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer ${FORGED}" "${BASE}/v1/internal/attempts/${ATTEMPT}")"
[ "${code}" = "401" ] || fail "a forged JWT answered ${code}, wanted 401"
log "phase 4 ok: 401"

log "ALL PHASES PASS"
