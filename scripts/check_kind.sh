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
#   5. pipeline — `run_all_full` runs as SEVEN pods, one per stage, each with
#                 its own Job object and one launch, and the lake ends up
#                 ledgered. This is the only phase where a stage must read what
#                 a previous stage's PROCESS wrote, which is the class of bug
#                 splitting a DAG across pods introduces (§11 Phase 3).
#   6. cutover   — `color.active` flips while a task is RUNNING, so the pod's
#                 control-plane callbacks land on a different er-server process
#                 than the one that launched it. The attempt must not notice:
#                 one launch, settles succeeded, exit 0, and a
#                 termination_reason that is either absent or the pod's own
#                 "Completed" — the reachable mid-run failure is `fenced`
#                 (three missed heartbeats), not `control_plane_unreachable`,
#                 which only the step-1 work-order loop can write. Phase 2 covers the
#                 dispatcher restarting; this covers the SERVING tier moving
#                 under an in-flight attempt, which is what a blue/green deploy
#                 does (D-BLUEGREEN).
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
# Every job route is DATASET-scoped since the datasets work landed
# (2026-11-04). `create_org` makes this dataset synchronously — "the org's
# one dataset exists from its first moment" — so there is nothing to wait for.
DS="default"
# Phase 5 repoints ${ORG} at its own pipeline org; phase 6 needs this one back.
MAINORG="${ORG}"

log() { echo "check_kind: $*" >&2; }
fail() { echo "check_kind: FAIL — $*" >&2; exit 1; }

# ---- the node's CPU budget ------------------------------------------------
# This exists because the failure it describes is UNREADABLE without it, and
# that cost three runs to work out. A pod the scheduler refuses sits `Pending`
# until whatever phase is waiting on it gives up, so an over-committed node
# reports `job … never reached succeeded (last: running)` — a timeout, about a
# job — when the truth is one line of `kubectl describe pod`:
#
#   FailedScheduling  0/1 nodes are available: 1 Insufficient cpu.
#   preemption: not eligible due to preemptionPolicy=Never
#
# The two numbers that turn the first message into the second are allocatable
# CPU and the sum of what is already requested. Neither was printed anywhere,
# so both are printed here — unconditionally, not only on failure, because the
# budget is the thing that has to be re-derived every time this moves and a
# green run is exactly when nobody records it. The arithmetic it answers to
# lives in infra/k8s/charts/er-platform/values-kind.yaml.
#
# Prints "<allocatable_millis> <requested_millis>".
cpu_budget() {
    kubectl --context "${CTX}" get nodes -o json >/tmp/er-kind-nodes.json
    kubectl --context "${CTX}" get pods --all-namespaces -o json >/tmp/er-kind-pods.json
    python3 - /tmp/er-kind-nodes.json /tmp/er-kind-pods.json <<'PY'
import json, sys


def millis(v):
    if v is None:
        return 0
    v = str(v)
    return int(float(v[:-1])) if v.endswith("m") else int(float(v) * 1000)


nodes = json.load(open(sys.argv[1]))["items"]
pods = json.load(open(sys.argv[2]))["items"]

alloc = sum(millis(n["status"]["allocatable"]["cpu"]) for n in nodes)

req = 0
for p in pods:
    # A finished pod holds no capacity, so counting it would overstate the
    # commitment — and this probe leaves a trail of Succeeded task pods.
    if p.get("status", {}).get("phase") in ("Succeeded", "Failed"):
        continue
    spec = p["spec"]
    app = sum(millis(c.get("resources", {}).get("requests", {}).get("cpu"))
              for c in spec.get("containers", []))
    # Init containers run before the app containers and never alongside them,
    # so the scheduler charges max(sum of app, largest init), not the sum. A
    # native sidecar (restartPolicy: Always) would break that; this chart has
    # none, and the web-api init container went away with PR10 anyway.
    init = max([millis(c.get("resources", {}).get("requests", {}).get("cpu"))
                for c in spec.get("initContainers", [])] or [0])
    req += max(app, init)

print(alloc, req)
PY
}

log_cpu_budget() {
    local alloc req
    read -r alloc req <<<"$(cpu_budget)"
    log "node CPU budget: ${req}m requested of ${alloc}m allocatable ($((alloc - req))m free)"
}

# assert_cpu_room <needed_millis> <what-it-is-for>
#
# Fails FAST and with the numbers, instead of letting the phase below discover
# the same thing as a timeout several minutes later.
assert_cpu_room() {
    local need="$1" what="$2" alloc req free
    read -r alloc req <<<"$(cpu_budget)"
    free=$((alloc - req))
    [ "${free}" -ge "${need}" ] || fail "${what} needs ${need}m of CPU and only ${free}m is free (${req}m requested of ${alloc}m allocatable). A pod the scheduler refuses sits Pending until this phase's wait expires, so leaving this to be discovered below would report a timeout rather than the scheduling refusal it is — see the budget table in infra/k8s/charts/er-platform/values-kind.yaml."
    log "${what}: needs ${need}m, ${free}m free"
}

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
# `/readyz`, NOT `/healthz`. `healthz` is a static literal — `Json(json!({
# "status": "ok" }))` — so it answers 200 while Postgres is unreachable and the
# process cannot serve a single DB-backed request. `readyz` is the real signal
# and is what the chart's readiness and startup probes read.
#
# CORRECTION, same day: when this changed from `/healthz` the comment here
# claimed the old gate was "the likely cause of the 2026-11-05 nightly failing
# in phase 1". It was not. The api() diagnostics added alongside it then
# printed the real answer on the next run — `POST .../jobs gave up after 5
# tries — HTTP 404` — a route that no longer existed, fixed below. Waiting on
# the literal was a real bug and is still worth fixing, but it was not that
# bug, and leaving the wrong cause written down here is how it becomes a
# premise. The lesson is the one that keeps recurring: a plausible cause found
# while fixing something else is a hypothesis, not a diagnosis.
#
# 90 ticks rather than 30 because `readyz` legitimately stays red through
# `ensure_schema` on a cold boot, which is the whole point of waiting for it.
for _ in $(seq 1 90); do
    curl -fsS "${BASE}/readyz" >/dev/null 2>&1 && break
    sleep 1
done
curl -fsS "${BASE}/readyz" >/dev/null \
    || fail "er-api never became READY through the port-forward (/readyz); /healthz says $(curl -s -o /dev/null -w '%{http_code}' "${BASE}/healthz")"

api() { # method path [json-body] — retried: `kubectl port-forward` drops the
    # odd connection mid-response, which surfaces as an empty body, not an
    # HTTP error. Five attempts cover it.
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
    # Every attempt failed. This comment used to say "a real 4xx/5xx still
    # fails fast because the body is empty on every attempt and the caller's
    # parse dies" — true, and a bad trade. `curl -fsS` prints no body on a
    # 4xx/5xx, so what reached the log was the CALLER's traceback:
    #
    #   File "<string>", line 1, in <module>
    #     import json,sys; print(json.load(sys.stdin)["id"])
    #   json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)
    #
    # That is how the 2026-11-05 nightly failed, and it names neither the
    # request nor the status — the two facts needed to tell "the API refused
    # this" from "the port-forward died". So re-ask ONCE without -f, purely to
    # report, and say so on stderr before failing.
    local probe_body code
    probe_body="$(mktemp)"
    if [ -n "${body}" ]; then
        code="$(curl -s -o "${probe_body}" -w '%{http_code}' -X "${method}" "${BASE}${path}" \
            -H "Authorization: Bearer ${OP_TOKEN}" -H "Content-Type: application/json" \
            -d "${body}" 2>/dev/null)" || true
    else
        code="$(curl -s -o "${probe_body}" -w '%{http_code}' -X "${method}" "${BASE}${path}" \
            -H "Authorization: Bearer ${OP_TOKEN}" 2>/dev/null)" || true
    fi
    # `000` is curl's own "never got an HTTP response", and it prints that
    # WHILE exiting non-zero — so this takes curl's code rather than
    # substituting one. An earlier version wrote `|| echo "000"`, which
    # appended a second 000 on exactly the failure it was meant to label:
    # the message read `HTTP 000000` and the parenthetical below never fired,
    # because the equality test no longer matched. Caught by running it
    # against a closed port instead of reasoning about it.
    [ -n "${code}" ] || code="000"
    # The distinction is the whole point: a refusal (the API answered and said
    # no) versus a dead port-forward (nothing answered at all).
    printf 'check_kind: %s %s gave up after 5 tries — HTTP %s%s: %s\n' \
        "${method}" "${path}" "${code}" \
        "$([ "${code}" = "000" ] && printf ' (no HTTP response — port-forward or pod gone)')" \
        "$(head -c 400 "${probe_body}" | tr '\n' ' ')" >&2
    rm -f "${probe_body}"
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
    api POST "/v1/orgs/${ORG}/datasets/${DS}/jobs" '{"kind":"lake_maintain"}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])'
}

job_state() {
    api GET "/v1/orgs/${ORG}/datasets/${DS}/jobs/$1" | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"])'
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

tasks_json() { api GET "/v1/orgs/${ORG}/datasets/${DS}/jobs/$1/tasks"; }

# The budget, before anything is enqueued: this is the steady state every
# phase below adds a task pod to, and recording it on a GREEN run is what makes
# the next move legible. A single task pod needs the executor class's cpu.
log_cpu_budget

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
        snap="$(api GET "/v1/orgs/${ORG}/datasets/${DS}/jobs/${id}/tasks" || true)"
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
api POST "/v1/orgs/${ORG}/datasets/${DS}/jobs/${JOB3}/cancel" >/dev/null
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

# ---- phase 5: a seven-stage pipeline, each stage its own pod ---------------
#
# The four phases above all drive single-task kinds, so none of them can show
# the thing most likely to break once a DAG is split across pods: a stage
# reading what the PREVIOUS STAGE'S PROCESS produced. In the monolith every
# stage shared one address space and one DuckDB handle; here `standardize` runs
# in a container that did not exist when `ingest` finished, and the only things
# that cross are the lake and the task's published output.
#
# That is not a hypothetical class of bug. `seed_lexicon` writes a
# SESSION-LOCAL table, which was invisible for as long as one process ran every
# stage and failed immediately once `init` and `standardize` were separate pods.
#
# A separate org, deliberately: the phases above depend on `lake_maintain`
# settling in seconds against an EMPTY lake, and seeding base_10 into the org
# they share would quietly change the window phases 2 and 3 are trying to
# catch. `pg_run_all_pipeline.rs` already holds the golden-partition
# comparison against a child-process double; what this adds is real pods, real
# scheduling and real per-pod token verification for all seven.
PORG="kindpipe"
log "phase 5: run_all_full as seven pods (org ${PORG})"

PDROP="${DATA_DIR}/drop/${PORG}"
mkdir -p "${PDROP}" "${DATA_DIR}/data/${PORG}"
cp "${ROOT}/rust/crates/er-cli/tests/fixtures/test.yaml" "${DATA_DIR}/configs/${PORG}.yaml"
cp "${ROOT}/rust/crates/er-cli/tests/fixtures/nickname_variants.csv" "${DATA_DIR}/configs/${PORG}.lexicon.csv"
cp "${ROOT}/fixtures/static/model_test_v1.json" "${DATA_DIR}/configs/${PORG}.model.json"
# A COPY of the frozen-TF fixture, never the fixture itself: `train` APPENDS
# its snapshot to that file, so pointing a tenant at the committed one would
# have the probe mutate the repo. It did, once, in the integration tier.
cp "${ROOT}/fixtures/static/model_test_v1.tf.csv" "${DATA_DIR}/configs/${PORG}.tf.csv"
for src in crm billing webforms; do
    cp "${ROOT}/fixtures/static/base_10/base/${src}.csv" "${PDROP}/${src}.csv"
done
chmod -R a+rwx "${DATA_DIR}/drop" "${DATA_DIR}/data" "${DATA_DIR}/configs"

api POST /v1/orgs "$(cat <<EOF
{
  "name": "${PORG}",
  "config_path": "/srv/er/configs/${PORG}.yaml",
  "drop_root": "/srv/er/drop/${PORG}",
  "env": {
    "ER_CONFIG_PATH": "/srv/er/configs/${PORG}.yaml",
    "ER_LAKE_CATALOG": "/srv/er/data/${PORG}/catalog.ducklake",
    "ER_LAKE_DATA_PATH": "/srv/er/data/${PORG}/",
    "ER_LEXICON_PATH": "/srv/er/configs/${PORG}.lexicon.csv",
    "ER_MODEL_PATH": "/srv/er/configs/${PORG}.model.json",
    "ER_TF_PATH": "/srv/er/configs/${PORG}.tf.csv",
    "ER_MODEL_VERSION": "v0001",
    "ER_TF_SNAPSHOT_ID": "01JWMDTFSNAP00000000000001"
  }
}
EOF
)" >/dev/null

PJOB="$(api POST "/v1/orgs/${PORG}/datasets/${DS}/jobs" '{"kind":"run_all_full"}' \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"

# A plain reassignment, not a `ORG=… wait_state` prefix: bash's scoping for an
# assignment prefixed onto a FUNCTION differs between POSIX and default mode,
# and the helpers interpolate ${ORG}. Phase 6 repoints it back to ${MAINORG}
# rather than inheriting this one, because its org needs the lake_maintain
# config rather than the pipeline fixture.
ORG="${PORG}"

# Wait for ANY terminal state, then dump the ledger, THEN require success.
# `wait_state … succeeded` would abort the moment the job settled `failed`,
# before anything printed the per-task rows — and in a seven-stage pipeline the
# only thing worth knowing is WHICH stage failed and with what `error_detail`.
# Generous on time: seven sequential pods, each paying a cold image start and a
# cold DuckDB open, on one kind node inside a laptop's docker VM.
pdeadline=$(( $(date +%s) + 1800 ))
pfinal=""
while [ "$(date +%s)" -lt "${pdeadline}" ]; do
    pfinal="$(job_state "${PJOB}")"
    case "${pfinal}" in succeeded|failed|canceled) break ;; esac
    sleep 5
done
tasks_json "${PJOB}" > /tmp/er-kind-tasks5.json
if [ "${pfinal}" != "succeeded" ]; then
    log "pipeline settled '${pfinal}' — the ledger, newest stage last:"
    python3 - /tmp/er-kind-tasks5.json <<'EOF' >&2 || true
import json, sys
for t in sorted(json.load(open(sys.argv[1]))["tasks"], key=lambda r: r["seq"]):
    if not t["live"]:
        continue
    print(f"  {t['task_id']:12} {t['state']:16} try={t['try_number']} "
          f"launches={t['launches']} class={t.get('resource_class')} "
          f"err={t.get('error_class')} {t.get('error_detail') or ''}")
EOF
    fail "the pipeline did not succeed (settled '${pfinal:-never settled}')"
fi
python3 - /tmp/er-kind-tasks5.json <<'EOF'
import json, sys

tasks = json.load(open(sys.argv[1]))["tasks"]
live = [t for t in tasks if t["live"]]
want = ["init", "ingest", "standardize", "match", "reconcile", "assemble", "finish"]
assert sorted(t["task_id"] for t in live) == sorted(want), sorted(t["task_id"] for t in live)

jobs = set()
for t in live:
    assert t["state"] == "succeeded", (t["task_id"], t["state"], t.get("error_detail"))
    assert t["launches"] == 1, f"{t['task_id']}: launches={t['launches']}"
    name = t["k8s_job_name"] or ""
    assert name.startswith("er-t-"), (t["task_id"], name)
    # A real pod, identified from the claims the control plane VERIFIED — the
    # child-process double in the integration tier cannot produce this.
    assert t["pod_uid"], f"{t['task_id']}: no verified pod uid"
    assert t["output"] is not None, f"{t['task_id']}: no durable output"
    jobs.add(name)
assert len(jobs) == 7, f"each stage needs its own Job object, got {len(jobs)}: {sorted(jobs)}"

# The handoff actually crossed the pod boundary. `init` published the run plan,
# and `standardize` pinned two references — the plan plus the corpus `ingest`
# landed, which only rehydrates if ingest's own pod published it.
init = next(t for t in live if t["task_id"] == "init")
assert init["output"]["plan"] == "run_plan", init["output"]
assert init["output"]["doc"]["mode"] == "full", init["output"]["doc"]
std = next(t for t in live if t["task_id"] == "standardize")
pins = std["input"]["pins"]
assert len(pins) == 2, f"standardize pinned {len(pins)} refs, wanted the plan + the landed corpus"
print(f"phase 5 ok: seven stages, seven Jobs, one launch each — {sorted(jobs)[0]} …")
EOF

# And the lake really holds the result: the run is ledgered under the model the
# work order carried. A pipeline whose pods all exited zero without writing
# anything would pass every assertion above and fail this one.
#
# Fetched WITHOUT `api`: this route is dataset-scoped and opens the lake in the
# API process, so a 503 is a distinguishable outcome and not a transport
# failure. `api` uses `curl -fsS`, which would exhaust its five retries on a
# 503 and then die under `set -e` having printed nothing at all — the body is
# the whole diagnosis here, so keep it.
runs_code="$(curl -s -o /tmp/er-kind-runs5.json -w '%{http_code}' \
    -H "Authorization: Bearer ${OP_TOKEN}" \
    "${BASE}/v1/orgs/${PORG}/datasets/${DS}/runs")"
if [ "${runs_code}" != "200" ]; then
    log "the runs read answered ${runs_code}: $(head -c 300 /tmp/er-kind-runs5.json)"
    fail "the control plane could not read back the lake the pods just wrote (HTTP ${runs_code})"
fi
python3 - /tmp/er-kind-runs5.json "${PJOB}" <<'EOF'
import json, sys

runs = json.load(open(sys.argv[1]))
runs = runs["runs"] if isinstance(runs, dict) else runs
mine = [r for r in runs if r.get("id") == sys.argv[2] or r.get("run_id") == sys.argv[2]]
assert len(mine) == 1, f"exactly one run row for the job, got {len(mine)} of {len(runs)}"
run = mine[0]
assert run.get("mode") == "full", run
assert run.get("model_version") == "v0001", run
print("phase 5 ok: the run is ledgered full @ v0001 — the pods wrote the lake")
EOF

# ---- phase 6: a colour cutover under an in-flight attempt -----------------
# The deploy's second phase moves `svc/er-api` from one colour to the other
# while work is running. A task pod's ONLY route to the control plane is that
# Service (`ER_SERVER_URL` in the task template is
# `http://er-api.<ns>.svc.cluster.local:8000`), so after the flip its
# heartbeats, its claim renewals and its finish call all land on a DIFFERENT
# er-server process than the one that launched it. If the cutover were visible
# to a running attempt it would fail paying work — a data-path failure caused
# by a serving-path deploy.
#
# What that failure would actually look like, because it decides the assertion:
# `exec-task` has TWO unreachability windows with different reasons, and only
# the second is reachable once a task is `running`.
#
#   step 1, work order   6 ticks x 10s = 60s, then control_plane_unreachable,
#                        exit 3, CHARGED TO THE LAUNCH BUDGET. A task already
#                        running is past this; a sibling that launches after
#                        the flip is not.
#   heartbeat thread     posts every 30s, fences after 3 consecutive transport
#                        misses (~90s). Step 6 then refuses to post a verdict
#                        and writes `fenced` — the work completed and the
#                        result is thrown away, which is the expensive one.
#
# So the assertion is an ALLOWLIST of undisrupted values — `None` or the
# literal "Completed" — rather than a single named reason or a bare `is None`.
# "Completed" is in it because the executor annotates an already-succeeded row
# with the POD's terminated reason, and that is what Kubernetes calls a
# container that exited 0; see the python block for why each of the two
# narrower guards this phase previously shipped could not work.
#
# Distinct from phase 2 in what it exercises: the dispatcher is NOT coloured and
# does not restart here, so adoption is not involved. What is being proved is
# that the attempt's callback path is stateless across the two pods.
log "phase 6: traffic cuts over while a task is in flight"
ORG="${MAINORG}"

svc_colour() {
    kubectl --context "${CTX}" -n "${NS}" get svc er-api \
        -o jsonpath='{.spec.selector.er\.dupezero\.com/color}'
}

helm_colour() { # extra --set args, applied with --reuse-values
    helm --kube-context "${CTX}" -n "${NS}" upgrade er-platform \
        "${ROOT}/infra/k8s/charts/er-platform" \
        --reuse-values "$@" --wait --timeout 5m >/dev/null
}

LIVE_COLOUR="$(svc_colour)"
# Not an assertion about which colour — an assertion that the chart HAS the
# colour dimension at all. A pre-PR9 chart leaves this empty, and every
# assertion below would then pass having cut nothing over.
[ -n "${LIVE_COLOUR}" ] || fail "svc/er-api carries no er.dupezero.com/color selector — the chart predates the colour dimension, so this phase would prove nothing"
case "${LIVE_COLOUR}" in
    blue)  IDLE_COLOUR=green ;;
    green) IDLE_COLOUR=blue ;;
    *)     fail "svc/er-api selects colour '${LIVE_COLOUR}', which is neither blue nor green" ;;
esac
log "live=${LIVE_COLOUR} idle=${IDLE_COLOUR}"

# This phase is the only one that needs THREE pods the steady state does not
# have, and on a two-vCPU runner that is where the budget runs out:
#
#   the idle colour's web-api   api.resources.requests.cpu
#   a task pod                  executor.classes.<class>.cpu
#   er-schema-migrate           100m — a pre-upgrade hook, so EVERY helm
#                               upgrade below re-fires it, including the
#                               `color.active` flip that happens while the
#                               task is in flight
#
# er-db-migrate is not in that list on purpose: it is guarded by
# `web.enabled`, which the kind profile turns off. Checked, rather than
# assumed, by rendering the profile.
#
# 450m is that sum at the kind profile's current numbers. Asserting it here
# costs one kubectl round trip and converts a five-minute `helm --wait`
# timeout, or a 420s wait on a job whose pod never started, into a sentence
# naming both figures. Raise it in step with values-kind.yaml.
assert_cpu_room 450 "phase 6 (the idle colour, a task pod, and the pre-upgrade hook)"

# Bring the idle colour up BEFORE enqueuing, so the only thing that happens
# inside the attempt's window is the selector flip. Its image comes from
# `images.*` through `er-platform.colorImage`'s `default` fallback, so it runs
# the same build as the live colour — which is the real deploy's steady state
# too, since a cutover differs from this only in the digest.
helm_colour --set "color.${IDLE_COLOUR}.replicas=1"
kubectl --context "${CTX}" -n "${NS}" rollout status "deploy/web-api-${IDLE_COLOUR}" --timeout=300s
log "idle colour ${IDLE_COLOUR} is up and ready, not yet serving"

JOB6=""
for attempt in 1 2 3; do
    candidate="$(enqueue)"
    if catch_running "${candidate}"; then
        JOB6="${candidate}"
        break
    fi
    log "job ${candidate} settled before the cutover window (attempt ${attempt}); trying a fresh one"
done
[ -n "${JOB6}" ] || fail "three jobs settled before a running task could be caught"

helm_colour --set "color.active=${IDLE_COLOUR}"
NOW_COLOUR="$(svc_colour)"
[ "${NOW_COLOUR}" = "${IDLE_COLOUR}" ] \
    || fail "cutover did not take: svc/er-api selects '${NOW_COLOUR}', wanted '${IDLE_COLOUR}'"
log "traffic moved ${LIVE_COLOUR} -> ${IDLE_COLOUR} with a task in flight"

# The probe's own port-forward is NOT affected and that is not a gap: `kubectl
# port-forward svc/...` resolves to one pod when the stream opens and does not
# re-resolve, so these reads keep landing on the old colour — which still runs,
# because retiring it is the deploy's third phase. Both colours read the same
# Postgres, so the ledger below is the same ledger either pod would serve. The
# flip is proved by the selector assertion above, not by where curl lands.
wait_state "${JOB6}" succeeded 420
tasks_json "${JOB6}" > /tmp/er-kind-tasks6.json
python3 - /tmp/er-kind-tasks6.json <<'EOF'
import json, sys

tasks = json.load(open(sys.argv[1]))["tasks"]
live = [t for t in tasks if t["live"]]
assert live, "no live task rows — the cutover window caught nothing"
for t in live:
    assert t["state"] == "succeeded", (t["task_id"], t["state"], t.get("error_detail"))
    # Subscript, not .get(): the assertion below is about a field's VALUE, so a
    # renamed or dropped field must raise KeyError here rather than let
    # `.get(...) != "fenced"` pass on a None that only means "no such key".
    # A gate that cannot fail is worse than no gate.
    assert "termination_reason" in t, (
        f"{t['task_id']}: no termination_reason field on the task row — the "
        "ledger shape changed and this phase would prove nothing"
    )
    # AN ALLOWLIST OF UNDISRUPTED VALUES, not `is None` and not a denylist of
    # named failures. Both of the obvious guards are wrong here, and this
    # phase has now shipped each of them:
    #
    #   `!= "control_plane_unreachable"` — the FIRST version. exec_task.rs
    #     writes that reason at exactly one site, the step-1 work-order loop,
    #     which a task already in `running` has passed. The precondition could
    #     not hold, so it passed whatever the cutover did.
    #   `is None` — the SECOND version, and what this replaces. It rested on
    #     "a succeeded task carries no reason at all", which is FALSE: the
    #     executor annotates a row that already holds a verdict with the
    #     POD's terminated reason (executor.rs record_evidence, guarded
    #     `state IN ('succeeded','skipped')`), and Kubernetes' reason for a
    #     container that exits 0 is the literal "Completed". So a perfectly
    #     undisrupted attempt ends up reading `termination_reason='Completed'`
    #     and the guard failed the phase for succeeding.
    #
    # It is also a RACE, which is why `== "Completed"` would be flaky rather
    # than correct: the annotation lands after the verdict, so a read here
    # legitimately sees None (not yet annotated) or "Completed" (annotated).
    # Both mean undisrupted; everything else means the cutover was visible.
    #
    # An allowlist is the FAIL-SAFE direction. A denylist of today's reasons
    # (fenced, sigterm, stale_start, structural_mismatch,
    # control_plane_unreachable, heartbeat_timeout, config, no_terminal_record)
    # would silently accept any reason added to exec_task.rs later — the
    # "exhaustive enumeration" defect, where the omission reads as permission.
    UNDISRUPTED = (None, "Completed")
    assert t["termination_reason"] in UNDISRUPTED, (
        f"{t['task_id']}: the cutover was visible to the attempt "
        f"(termination_reason={t['termination_reason']!r}, "
        f"error_detail={t.get('error_detail')!r}) — undisrupted is None or "
        f"'Completed'; 'fenced' means the heartbeat missed three posts across "
        "the flip; 'control_plane_unreachable' means a sibling could not fetch "
        "its work order after it"
    )
    # Corroborates "Completed" instead of trusting it. The reason above is the
    # POD's, so a zero exit is the independent half of the same claim, and a
    # succeeded row carrying a nonzero exit is incoherent however it got there.
    # `None` is allowed for the same race as above — unannotated, not yet read.
    assert t["exit_code"] in (None, 0), (
        f"{t['task_id']}: state is succeeded but exit_code={t['exit_code']!r} "
        f"(termination_reason={t['termination_reason']!r})"
    )
    # A relaunch would mean two processes writing one lake — the same invariant
    # phase 2 protects, re-checked because a cutover is a second way to lose it.
    assert t["launches"] == 1, (
        f"{t['task_id']}: launches={t['launches']} — a cutover must NOT relaunch "
        "a live attempt"
    )
# Names what was actually checked. The old wording claimed "no
# control_plane_unreachable", which is the reason the FIRST version of this
# guard wrongly keyed on and which a running attempt cannot reach — a green
# line asserting the wrong thing is how that guard survived review.
print("phase 6 ok: the attempt survived the cutover — one launch, exit 0, "
      "and no disrupting termination_reason")
EOF

# Leave the release as phase 1 found it, so the probe is re-runnable and a
# later `kind_env.sh up` does not inherit a surprising active colour. Order
# matters: flip back FIRST, then retire — the reverse would blackhole the
# Service for as long as the scale-down took.
helm_colour --set "color.active=${LIVE_COLOUR}"
helm_colour --set "color.${IDLE_COLOUR}.replicas=0"
RESTORED="$(svc_colour)"
[ "${RESTORED}" = "${LIVE_COLOUR}" ] \
    || fail "restore failed: svc/er-api selects '${RESTORED}', wanted '${LIVE_COLOUR}'"
log "phase 6 cleanup: restored to ${LIVE_COLOUR}, ${IDLE_COLOUR} retired"

log "ALL PHASES PASS"
