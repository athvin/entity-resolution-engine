#!/usr/bin/env bash
# Measure how long a pipeline run takes to *start*, on the live cluster.
#
# This is the harness behind docs/infrastructure.md §18.16 — "cold-node image
# pull time for the current image", the deciding measurement that
# infrastructure-decisions.md D-PULL and plan-to-aws.md 2.3 both wait on and
# that nothing in this repo has ever produced. The two numbers in circulation
# (~90 s for a probe pod, 77 s pod-to-complete) are prose in a plan checkbox
# with no artifact and no method behind them; this script is the method.
#
# It runs one or more arms, each 1..N trials, and emits a record following the
# docs/measurements/*.json convention. Run it twice over the SAME image — once
# per arm — and the two become a before/after comparison:
#
#   scripts/measure_runner_startup.sh --image <registry>/er-api:<sha> --arm before
#   scripts/measure_runner_startup.sh --image <registry>/er-api:<sha> --arm after
#
# ONE IMAGE, NOT TWO. The `before` arm forces `ER_DUCKDB_EXTENSION_DIR` empty in
# the probe container, which makes `SessionOptions::with_baked_extensions` skip
# and restores the previous code path exactly: `attach_ducklake` issues its
# INSTALL, autoinstall stays on, and DuckDB fetches over the network. Same
# image, same binary, same commit, same node — the only variable is whether the
# baked directory is consulted. Comparing two images instead would vary every
# byte, and the nearest comparable main-built image turned out to predate the
# per-run lake-schema reconcile, so its probe exercised less of the session
# path. Image *size* is answered from the registry, not from a probe.
#
# THE ARMS
#
#   x1  Container start -> DuckLake attached and queried, on a fresh pod on the
#       always-on app pool. This is the extension term: the image is built
#       without DuckDB's `parquet`/`json` features and with autoinstall and
#       autoload defaulting to on, so a cold process fetches ducklake, parquet,
#       json and icu from extensions.duckdb.org over the NAT gateway.
#   x2  The same probe with egress denied. A binary result, not a timing: the
#       baseline MUST fail (it needs the extension repo) and a baked image MUST
#       succeed. This is what proves the dependency is gone rather than merely
#       faster.
#   n1  The cold-node split: NodeClaim created -> Launched -> Registered ->
#       Ready, then pod Scheduled -> Pulling -> Pulled -> Started. The pull term
#       is taken from the kubelet's own `Pulled` event message, which carries
#       sub-second precision; the node-lifecycle conditions are whole seconds,
#       which is all Kubernetes records.
#   n2  Warm-node reuse: a second job submitted after the first finishes, to see
#       whether it lands on the still-running node or pays a fresh cold start.
#       This is the `consolidateAfter` arm, so the script sets that timer
#       explicitly per run rather than inheriting it.
#
# WHY A PROBE POOL. n1/n2 need a cold node on a runner-shaped pool, and
# `runner-sm` cannot provide one here: the applied EC2 Standard vCPU quotas are
# 8 for spot and 8 for on-demand, and the app + system nodes hold 2 each, so a
# `c6id.2xlarge` (8 vCPU) never places. infra/k8s/nodepools/measure/runner-probe.yaml
# is runner-sm with `instance-size: xlarge` and nothing else changed; its header
# records what that does and does not make faithful.
#
# Requires: kubectl against the dev cluster, jq, and (for image sizes) aws with
# a profile that can read ECR.
set -euo pipefail

IMAGE=""
ARM=""
TRIALS=3
NAMESPACE="${ER_MEASURE_NS:-rustdev}"
ARMS="x1,x2,n1,n2"
OUT=""
CONTEXT="${ER_DEV_CONTEXT:-er-dev}"
CONFIG_FILE="configs/test.yaml"
KEEP=0

die() { printf 'measure: %s\n' "$*" >&2; exit 2; }
log() { printf 'measure: %s\n' "$*" >&2; }

usage() {
  sed -n '2,50p' "$0" | sed 's/^# \{0,1\}//'
  cat >&2 <<'EOF'

Usage: scripts/measure_runner_startup.sh --image REF --arm before|after
         [--trials N] [--arms x1,x2,n1,n2] [--out FILE]
         [--namespace NS] [--context CTX] [--keep]

  --image REF    Full image reference to probe (required).
  --arm NAME     Which side of the comparison this run is (required).
  --trials N     Repetitions per arm (default 3). Every trial is recorded;
                 nothing is averaged away, because a cold node launch is
                 high-variance and the spread is part of the result.
  --arms LIST    Comma-separated subset of x1,x2,n1,n2 (default all).
  --out FILE     JSON to write/update (default
                 docs/measurements/runner-startup-<UTC date>.json).
  --keep         Leave the probe NodePool and Jobs in place (for debugging).
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --image) IMAGE="${2:-}"; shift 2 ;;
    --arm) ARM="${2:-}"; shift 2 ;;
    --trials) TRIALS="${2:-}"; shift 2 ;;
    --arms) ARMS="${2:-}"; shift 2 ;;
    --out) OUT="${2:-}"; shift 2 ;;
    --namespace) NAMESPACE="${2:-}"; shift 2 ;;
    --context) CONTEXT="${2:-}"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument $1 (try --help)" ;;
  esac
done

[ -n "$IMAGE" ] || { usage; die "--image is required"; }
case "$ARM" in before|after) ;; *) die "--arm must be 'before' or 'after'" ;; esac
command -v kubectl >/dev/null || die "kubectl not found"
command -v jq >/dev/null || die "jq not found"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
[ -f "$CONFIG_FILE" ] || die "$CONFIG_FILE not found (run from the repo root)"
PROBE_POOL="infra/k8s/nodepools/measure/runner-probe.yaml"
[ -f "$PROBE_POOL" ] || die "$PROBE_POOL not found"

TODAY="$(date -u +%Y%m%d)"
OUT="${OUT:-docs/measurements/runner-startup-$TODAY.json}"
K=(kubectl --context "$CONTEXT")
RUNTAG="$(date -u +%H%M%S)$$"
WORK="$(mktemp -d)"
mkdir -p "$WORK/trials"
trap 'cleanup' EXIT

CREATED_POOL=0

cleanup() {
  local rc=$?
  if [ "$KEEP" = 1 ]; then
    log "--keep: leaving probe objects in place"
    return $rc
  fi
  log "cleaning up probe objects"
  "${K[@]}" -n "$NAMESPACE" delete job -l "er.dupezero.com/measure=$RUNTAG" \
    --ignore-not-found --wait=false >/dev/null 2>&1 || true
  "${K[@]}" -n "$NAMESPACE" delete pod -l "er.dupezero.com/measure=$RUNTAG" \
    --ignore-not-found --wait=false >/dev/null 2>&1 || true
  "${K[@]}" -n "$NAMESPACE" delete networkpolicy -l "er.dupezero.com/measure=$RUNTAG" \
    --ignore-not-found >/dev/null 2>&1 || true
  "${K[@]}" -n "$NAMESPACE" delete configmap "probe-config-$RUNTAG" \
    --ignore-not-found >/dev/null 2>&1 || true
  if [ "$CREATED_POOL" = 1 ]; then
    # Deleting the NodePool drains its NodeClaims, which is how the probe node
    # goes away; without this a measurement failure leaves an instance running.
    "${K[@]}" delete -f "$PROBE_POOL" --ignore-not-found >/dev/null 2>&1 || true
  fi
  rm -rf "$WORK"
  return $rc
}

# ---------------------------------------------------------------- preflight

preflight() {
  "${K[@]}" version --request-timeout=20s >/dev/null 2>&1 \
    || die "cannot reach the cluster on context '$CONTEXT' (aws sso login?)"
  "${K[@]}" get ns "$NAMESPACE" >/dev/null 2>&1 \
    || die "namespace '$NAMESPACE' does not exist"
  log "context=$CONTEXT namespace=$NAMESPACE image=$IMAGE arm=$ARM trials=$TRIALS"

  # The vCPU ceiling is the reason the probe pool exists; say so out loud so a
  # Pending pod is never mistaken for a slow one.
  local used
  used="$("${K[@]}" get nodes -o json \
    | jq '[.items[].status.capacity.cpu | tonumber] | add // 0')"
  log "cluster currently holds ${used} vCPU of node capacity (quota is 8 spot / 8 on-demand)"
}

# The probe's config travels in a ConfigMap: the image ships binaries only, and
# every `er` subcommand takes --config.
ensure_config() {
  "${K[@]}" -n "$NAMESPACE" create configmap "probe-config-$RUNTAG" \
    --from-file="config.yaml=$CONFIG_FILE" >/dev/null
}

# The probe itself. It opens a brand-new local DuckLake (a dev lake "is just a
# directory", per docs/runbook.md) and runs the read-only `er doctor` report,
# which is enough to force the whole session path: open the DuckDB connection,
# apply the session options, load the extensions, ATTACH the catalog, reconcile
# the lake schema and query every relation. The marks are the process's own
# millisecond clock, so the extension term is measured at better than
# Kubernetes' one-second resolution.
#
# SUCCESS IS NOT THE EXIT CODE. `er doctor` exits 1 against a lake with no runs
# in it — measured locally, both with and without baked extensions — so the exit
# status says nothing about whether the session opened. The report itself is the
# signal: it is JSON ending in a `"tenant"` field, and it cannot be produced
# without a loaded ducklake extension and an attached catalog. PROBE_OK is what
# the x2 arm's verdict is built on; PROBE_RC is kept alongside it for the record.
probe_script() {
  cat <<'SH'
set -u
# Run the probe TWICE, against two separate fresh lakes.
#
# This is what isolates the extension fetch without needing to instrument
# DuckDB. The first run pays whatever the process has to download; the second
# cannot, because by then the binaries are in the extension directory (either
# the one baked into the image, or ~/.duckdb populated by the first run). So:
#
#   baseline image -> run1 includes the download, run2 does not; the delta IS
#                     the fetch, measured rather than inferred
#   baked image    -> run1 ~= run2, because nothing is ever fetched
#
# A one-run probe cannot tell "the fetch was fast" from "the fetch did not
# happen", which is exactly the ambiguity that showed up the first time this
# ran: 740 ms for a reported ~33 MB of extension felt wrong and there was no
# way to check from the outside.
probe_once() {
  out="$1"; err="$2"; lake="$3"
  mkdir -p "$lake" "${HOME:-/tmp/home}"
  er --catalog "$lake/probe.ducklake" --data-path "$lake/data" \
     --config /probe/config.yaml doctor >"$out" 2>"$err"
}

t0=$(date +%s%3N)
rc1=0
probe_once /tmp/p1.out /tmp/p1.err /tmp/lake1 || rc1=$?
t1=$(date +%s%3N)
rc2=0
probe_once /tmp/p2.out /tmp/p2.err /tmp/lake2 || rc2=$?
t2=$(date +%s%3N)

# `er doctor` exits non-zero against a lake with no runs in it, so the report
# itself is the success signal. It cannot be produced without a loaded ducklake
# extension and an attached, reconciled catalog.
ok=0
grep -q '"tenant"' /tmp/p1.out 2>/dev/null && ok=1
relations=$(grep -c '"name"' /tmp/p1.out 2>/dev/null || echo 0)

echo "PROBE_T0=$t0"
echo "PROBE_T1=$t1"
echo "PROBE_RC=$rc1"
echo "PROBE_OK=$ok"
echo "PROBE_RELATIONS=$relations"
echo "PROBE_ELAPSED_MS=$((t1 - t0))"
echo "PROBE_SECOND_ELAPSED_MS=$((t2 - t1))"
echo "PROBE_SECOND_RC=$rc2"
# first - second, i.e. what the first run paid that the second did not.
echo "PROBE_FETCH_MS=$(( (t1 - t0) - (t2 - t1) ))"
echo "--- probe stderr (run 1) ---"
cat /tmp/p1.err 2>/dev/null || true
echo "--- probe stderr (run 2) ---"
cat /tmp/p2.err 2>/dev/null || true
# Exit 0 on a successful probe so the Job reaches Complete; the arms read
# PROBE_OK, and a non-zero exit here would only mean "the report never came out".
[ "$ok" = 1 ] || exit 1
SH
}

# One Job manifest. `runner=1` adds the runner taint toleration and pins the
# pool, so x1/x2 land on the always-on app node and n1/n2 land on a cold probe
# node. backoffLimit 0 so a failure is one pod and one pod only — the same
# reason §6.2 insists on it for real runs.
#
# THE BEFORE/AFTER CONTROL IS AN ENV VAR, NOT A SECOND IMAGE. On the `before`
# arm the probe container gets `ER_DUCKDB_EXTENSION_DIR=""`, which makes
# `SessionOptions::with_baked_extensions` skip and restores the previous code
# path exactly: `attach_ducklake` issues its `INSTALL`, autoinstall stays on,
# DuckDB looks in ~/.duckdb instead of the baked directory, and the extensions
# are fetched over the network.
#
# This is a better experiment than comparing two images, and not just a cheaper
# one. Same image, same binary, same commit, same node, same probe — the only
# thing that differs is whether the baked directory is consulted, which is the
# variable under test. Two images would have differed in every byte, and the
# only image built from a commit comparable to this branch turned out to predate
# the per-run lake-schema reconcile, so its probe exercised less of the session
# path.
#
# Image *size* is a separate question and is answered from the registry
# (`image_detail`), not from a probe.
job_manifest() {
  local name="$1" runner="$2" netpol="$3"
  local sched=""
  # Empty string, not unset: the chart/image sets this, so overriding it to
  # empty is how a pod opts back out. `with_baked_extensions` filters empties.
  local baked_env=""
  if [ "$ARM" = before ]; then
    baked_env=$(cat <<'YAML'
            - name: ER_DUCKDB_EXTENSION_DIR
              value: ""
YAML
)
  fi
  if [ "$runner" = 1 ]; then
    sched=$(cat <<'YAML'
      nodeSelector:
        karpenter.sh/nodepool: runner-probe
      tolerations:
        - key: er.dupezero.com/runner
          operator: Equal
          value: "true"
          effect: NoSchedule
YAML
)
  fi
  cat <<YAML
apiVersion: batch/v1
kind: Job
metadata:
  name: $name
  namespace: $NAMESPACE
  labels:
    er.dupezero.com/measure: "$RUNTAG"
spec:
  backoffLimit: 0
  completions: 1
  parallelism: 1
  activeDeadlineSeconds: 900
  template:
    metadata:
      labels:
        er.dupezero.com/measure: "$RUNTAG"
        er.dupezero.com/probe: "${netpol}"
      annotations:
        karpenter.sh/do-not-disrupt: "true"
    spec:
      restartPolicy: Never
      # The namespace enforces PodSecurity baseline but warns and audits
      # against restricted, so a probe without this is admitted and then
      # generates four warnings on every single trial. The image already runs
      # as uid 1001 (rust/Dockerfile), so satisfying restricted costs nothing
      # and keeps the measurement from polluting the audit log.
      # (No backticks in this heredoc: its delimiter is unquoted, so the shell
      # would command-substitute them.)
      securityContext:
        runAsNonRoot: true
        seccompProfile:
          type: RuntimeDefault
$sched
      containers:
        - name: probe
          image: $IMAGE
          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop: ["ALL"]
          command: ["/bin/sh", "-c"]
          args:
            - |
$(probe_script | sed 's/^/              /')
          env:
            # HOME so DuckDB's default ~/.duckdb is a writable, EMPTY directory
            # per container. The image's user has /app as its home, and on the
            # before arm the fetch has to land somewhere it will not be found
            # again by the next trial.
            - name: HOME
              value: /tmp/home
$baked_env
          volumeMounts:
            - name: probe-config
              mountPath: /probe
          resources:
            requests:
              cpu: 500m
              memory: 1Gi
              ephemeral-storage: 2Gi
            limits:
              cpu: "2"
              memory: 2Gi
              ephemeral-storage: 8Gi
      volumes:
        - name: probe-config
          configMap:
            name: probe-config-$RUNTAG
YAML
}

# Deny every egress except cluster DNS. The baseline image cannot complete the
# probe under this policy (it has to reach extensions.duckdb.org); a baked image
# must not notice it at all.
netpol_manifest() {
  cat <<YAML
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: probe-deny-egress-$RUNTAG
  namespace: $NAMESPACE
  labels:
    er.dupezero.com/measure: "$RUNTAG"
spec:
  podSelector:
    matchLabels:
      er.dupezero.com/probe: "denied"
  policyTypes: [Egress]
  egress:
    - to:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: kube-system
      ports:
        - protocol: UDP
          port: 53
        - protocol: TCP
          port: 53
YAML
}

# Wait for a Job to reach a terminal state. Returns 0 on Complete, 1 on Failed,
# 2 on timeout — a timeout is reported, never silently treated as a failure,
# because "the pod never scheduled" and "the probe errored" are different
# findings.
wait_job() {
  local name="$1" deadline=$(( $(date +%s) + 900 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    local succeeded failed
    succeeded="$("${K[@]}" -n "$NAMESPACE" get job "$name" \
      -o jsonpath='{.status.succeeded}' 2>/dev/null || true)"
    failed="$("${K[@]}" -n "$NAMESPACE" get job "$name" \
      -o jsonpath='{.status.failed}' 2>/dev/null || true)"
    [ "${succeeded:-0}" = "1" ] && return 0
    [ "${failed:-0}" != "0" ] && [ -n "${failed:-}" ] && return 1
    sleep 2
  done
  return 2
}

pod_of() {
  "${K[@]}" -n "$NAMESPACE" get pods -l "job-name=$1" \
    -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true
}

# ------------------------------------------------------------------- arms

# Everything the cluster recorded about one probe pod, as a JSON object.
#
# Every extraction is written to yield null rather than raise: `capture` throws
# on no-match, and a pod that never scheduled has no containerStatuses at all,
# so each is wrapped. A harvest that errored would lose the trial it was trying
# to describe — exactly the case (a Pending pod) most worth recording.
harvest_pod() {
  local pod="$1"
  local pj ej logs
  pj="$("${K[@]}" -n "$NAMESPACE" get pod "$pod" -o json 2>/dev/null || echo '{}')"
  ej="$("${K[@]}" -n "$NAMESPACE" get events \
    --field-selector "involvedObject.name=$pod" -o json 2>/dev/null || echo '{"items":[]}')"
  logs="$("${K[@]}" -n "$NAMESPACE" logs "$pod" --tail=60 2>/dev/null || true)"

  jq -n --argjson pod "$pj" --argjson ev "$ej" --arg logs "$logs" '
    # The first timestamp of the event with this reason, or null.
    def ev_at($reason):
      [ $ev.items[]? | select(.reason == $reason)
        | (.eventTime // .firstTimestamp // .lastTimestamp) ] | first // null;
    # A numeric field the kubelet put in a Pulled message, or null.
    def pull_num($re):
      [ $ev.items[]? | select(.reason == "Pulled") | .message
        | (try (capture($re) | .s | tonumber) catch empty) ] | first // null;
    # One "KEY=<int>" mark the probe printed, or null if the probe never ran.
    def mark($k):
      [ $logs | split("\n")[] | select(startswith($k + "="))
        | (try (split("=")[1] | tonumber) catch empty) ] | first // null;
    def cs: ($pod.status.containerStatuses[0]? // {});
    {
      pod: ($pod.metadata.name? // null),
      node: ($pod.spec.nodeName? // null),
      phase: ($pod.status.phase? // null),
      scheduled_at: ([ ($pod.status.conditions[]? | select(.type == "PodScheduled")
                        | .lastTransitionTime) ] | first // null),
      pulling_at: ev_at("Pulling"),
      pulled_at: ev_at("Pulled"),
      started_at: (cs.state.terminated.startedAt? // cs.state.running.startedAt? // null),
      finished_at: (cs.state.terminated.finishedAt? // null),
      exit_code: (cs.state.terminated.exitCode? // null),
      # "Successfully pulled image ... in 12.345s (13.1s including waiting)".
      # The first number is the pull; the second adds time queued behind other
      # pulls on the same node, which on a cold single-pod node is the same.
      pull_seconds: pull_num("in (?<s>[0-9.]+)s"),
      pull_including_wait_seconds: pull_num("\\((?<s>[0-9.]+)s including waiting\\)"),
      probe_elapsed_ms: mark("PROBE_ELAPSED_MS"),
      probe_rc: mark("PROBE_RC"),
      probe_ok: (mark("PROBE_OK") == 1),
      probe_relations: mark("PROBE_RELATIONS"),
      probe_second_elapsed_ms: mark("PROBE_SECOND_ELAPSED_MS"),
      probe_second_rc: mark("PROBE_SECOND_RC"),
      # First run minus second run: what the first paid and the second
      # did not. On a baked image this should collapse toward zero.
      probe_fetch_ms: mark("PROBE_FETCH_MS"),
      probe_tail: ($logs | split("\n") | map(select(length > 0)) | .[-30:])
    }'
}

arm_x1() {
  local trial="$1"
  local name="probe-x1-$RUNTAG-$trial"
  log "x1 trial $trial: first-query latency on the app pool"
  job_manifest "$name" 0 "open" | "${K[@]}" apply -f - >/dev/null
  local rc=0; wait_job "$name" || rc=$?
  local pod; pod="$(pod_of "$name")"
  local h; h="$(harvest_pod "$pod")"
  jq -n --argjson h "$h" --arg trial "$trial" --argjson rc "$rc" \
    '{arm: "x1", trial: ($trial | tonumber), job_result: $rc, pod: $h}' \
    > "$WORK/trials/x1-$trial.json"
  "${K[@]}" -n "$NAMESPACE" delete job "$name" --wait=false >/dev/null 2>&1 || true
}

arm_x2() {
  local trial="$1"
  local name="probe-x2-$RUNTAG-$trial"
  log "x2 trial $trial: the same probe with egress denied"
  netpol_manifest | "${K[@]}" apply -f - >/dev/null
  job_manifest "$name" 0 "denied" | "${K[@]}" apply -f - >/dev/null
  local rc=0; wait_job "$name" || rc=$?
  local pod; pod="$(pod_of "$name")"
  local h; h="$(harvest_pod "$pod")"
  # The interpretation is deliberately recorded alongside the raw result: for
  # this arm a *failure* is the expected baseline outcome, and a reader of the
  # artifact should not have to rediscover that.
  jq -n --argjson h "$h" --arg trial "$trial" --argjson rc "$rc" --arg arm "$ARM" '
    {
      arm: "x2", trial: ($trial | tonumber), job_result: $rc, pod: $h,
      expectation: (if $arm == "before"
                    then "fails: the image must fetch its DuckDB extensions over the network"
                    else "succeeds: the extensions are baked, so no egress is needed" end),
      # The verdict reads the report, not the exit status — see probe_script.
      probe_produced_report: ($h.probe_ok // false),
      meets_expectation: (if $arm == "before"
                          then (($h.probe_ok // false) | not)
                          else ($h.probe_ok // false) end)
    }' > "$WORK/trials/x2-$trial.json"
  "${K[@]}" -n "$NAMESPACE" delete job "$name" --wait=false >/dev/null 2>&1 || true
}

# Apply the probe pool with a given consolidateAfter, and make sure no node of
# it is already running so the next pod really is cold.
reset_probe_pool() {
  local consolidate="$1"
  CREATED_POOL=1
  sed "s/^    consolidateAfter: .*/    consolidateAfter: $consolidate/" "$PROBE_POOL" \
    | "${K[@]}" apply -f - >/dev/null
  "${K[@]}" delete nodeclaim -l "karpenter.sh/nodepool=runner-probe" \
    --ignore-not-found --wait=true --timeout=5m >/dev/null 2>&1 || true
  # Karpenter needs a moment to observe the empty pool.
  sleep 5
}

# The NodeClaim lifecycle for the claim that carried `node`. Karpenter stamps
# each stage as a status condition: Launched (the EC2 fleet call returned),
# Registered (the kubelet joined), Initialized (startup taints cleared), Ready.
# Those four plus creationTimestamp are the §18.16 split, at the one-second
# resolution Kubernetes records them in.
harvest_nodeclaim() {
  local node="$1"
  local nj
  nj="$("${K[@]}" get nodeclaims -o json 2>/dev/null \
    | jq --arg n "$node" '[.items[]? | select(.status.nodeName == $n)] | first // {}' \
    || echo '{}')"
  jq -n --argjson nc "$nj" '
    def cond($t):
      [ $nc.status.conditions[]? | select(.type == $t) | .lastTransitionTime ] | first // null;
    {
      name: ($nc.metadata.name? // null),
      instance: ($nc.metadata.labels["node.kubernetes.io/instance-type"]? // null),
      capacity_type: ($nc.metadata.labels["karpenter.sh/capacity-type"]? // null),
      zone: ($nc.metadata.labels["topology.kubernetes.io/zone"]? // null),
      created_at: ($nc.metadata.creationTimestamp? // null),
      launched_at: cond("Launched"),
      registered_at: cond("Registered"),
      initialized_at: cond("Initialized"),
      ready_at: cond("Ready")
    }'
}

arm_n1() {
  local trial="$1"
  local name="probe-n1-$RUNTAG-$trial"
  log "n1 trial $trial: cold-node split on the probe pool"
  reset_probe_pool "30s"
  local submit; submit="$(date -u +%s)"
  job_manifest "$name" 1 "open" | "${K[@]}" apply -f - >/dev/null
  local rc=0; wait_job "$name" || rc=$?
  local pod; pod="$(pod_of "$name")"
  local h; h="$(harvest_pod "$pod")"
  local node; node="$(jq -r '.node // ""' <<<"$h")"
  local nc; nc="$(harvest_nodeclaim "$node")"
  jq -n --argjson h "$h" --argjson nc "$nc" --arg trial "$trial" \
        --argjson rc "$rc" --argjson submit "$submit" '
    {arm: "n1", trial: ($trial | tonumber), job_result: $rc,
     submitted_at_epoch: $submit, nodeclaim: $nc, pod: $h}' \
    > "$WORK/trials/n1-$trial.json"
  "${K[@]}" -n "$NAMESPACE" delete job "$name" --wait=false >/dev/null 2>&1 || true
}

arm_n2() {
  local trial="$1" consolidate="$2"
  local first="probe-n2a-$RUNTAG-$trial" second="probe-n2b-$RUNTAG-$trial"
  log "n2 trial $trial: warm-node reuse at consolidateAfter=$consolidate"
  reset_probe_pool "$consolidate"
  job_manifest "$first" 1 "open" | "${K[@]}" apply -f - >/dev/null
  local rc1=0; wait_job "$first" || rc1=$?
  local pod1; pod1="$(pod_of "$first")"
  local node1; node1="$("${K[@]}" -n "$NAMESPACE" get pod "$pod1" \
    -o jsonpath='{.spec.nodeName}' 2>/dev/null || true)"
  # The gap is the point: long enough that a 30s timer has reclaimed the node
  # and a 10m timer has not. This is the back-to-back case §3 of the research
  # doc calls "the worst user-visible case".
  log "  first job done on ${node1:-?}; waiting 120s before the second"
  sleep 120
  job_manifest "$second" 1 "open" | "${K[@]}" apply -f - >/dev/null
  local rc2=0; wait_job "$second" || rc2=$?
  local pod2; pod2="$(pod_of "$second")"
  local h2; h2="$(harvest_pod "$pod2")"
  local node2; node2="$(jq -r '.node // ""' <<<"$h2")"
  local nc2; nc2="$(harvest_nodeclaim "$node2")"
  jq -n --argjson h2 "$h2" --argjson nc2 "$nc2" --arg trial "$trial" \
        --arg consolidate "$consolidate" --arg node1 "${node1:-}" \
        --argjson rc1 "$rc1" --argjson rc2 "$rc2" '
    {
      arm: "n2", trial: ($trial | tonumber), consolidate_after: $consolidate,
      first_job_result: $rc1, second_job_result: $rc2,
      first_node: $node1, second_node: ($h2.node // null),
      reused_node: (($node1 != "") and ($h2.node == $node1)),
      second_nodeclaim: $nc2, second_pod: $h2
    }' > "$WORK/trials/n2-$trial.json"
  "${K[@]}" -n "$NAMESPACE" delete job "$first" "$second" --wait=false >/dev/null 2>&1 || true
}

# Compressed image size, straight from the registry. Not a timing, but it is
# the term the pull time is a function of, and §18.16 has never had it either.
image_detail() {
  local repo tag out
  repo="$(printf '%s' "$IMAGE" | sed 's#.*/##; s#:.*##')"
  tag="$(printf '%s' "$IMAGE" | sed 's#.*:##')"
  if ! command -v aws >/dev/null 2>&1; then
    jq -n --arg r "$repo" --arg t "$tag" \
      '{repository: $r, tag: $t, compressed_bytes: null,
        note: "aws CLI unavailable; size not recorded"}' > "$WORK/image.json"
    return
  fi
  if out="$(aws ecr describe-images --repository-name "$repo" \
        --region "${ECR_REGION:-us-east-2}" --image-ids "imageTag=$tag" \
        --output json 2>/dev/null)"; then
    jq -n --arg r "$repo" --arg t "$tag" --argjson d "$out" \
      '{repository: $r, tag: $t,
        compressed_bytes: ($d.imageDetails[0].imageSizeInBytes? // null),
        pushed_at: ($d.imageDetails[0].imagePushedAt? // null),
        digest: ($d.imageDetails[0].imageDigest? // null)}' > "$WORK/image.json"
  else
    jq -n --arg r "$repo" --arg t "$tag" \
      '{repository: $r, tag: $t, compressed_bytes: null,
        note: "ECR describe-images failed (credentials or missing tag)"}' > "$WORK/image.json"
  fi
}

# ------------------------------------------------------------------ driver

preflight
ensure_config

IFS=',' read -r -a WANTED <<< "$ARMS"
for a in "${WANTED[@]}"; do
  case "$a" in
    x1) for i in $(seq 1 "$TRIALS"); do arm_x1 "$i"; done ;;
    x2) for i in $(seq 1 "$TRIALS"); do arm_x2 "$i"; done ;;
    n1) for i in $(seq 1 "$TRIALS"); do arm_n1 "$i"; done ;;
    n2)
      # The before arm measures today's 30s; the after arm measures 10m. Both
      # also record the timer, so the artifact is readable without this script.
      if [ "$ARM" = before ]; then c=30s; else c=10m; fi
      for i in $(seq 1 "$TRIALS"); do arm_n2 "$i" "$c"; done ;;
    *) die "unknown arm '$a'" ;;
  esac
done

image_detail

# ------------------------------------------------------------------ record

NODE_ENVELOPE="$("${K[@]}" get nodes -o json | jq '[.items[] | {
  name: .metadata.name,
  instance: .metadata.labels["node.kubernetes.io/instance-type"],
  capacity_type: .metadata.labels["karpenter.sh/capacity-type"],
  nodepool: .metadata.labels["karpenter.sh/nodepool"],
  arch: .metadata.labels["kubernetes.io/arch"],
  kubelet: .status.nodeInfo.kubeletVersion,
  os_image: .status.nodeInfo.osImage
}]')"

TRIAL_JSON="$(cat "$WORK"/trials/*.json 2>/dev/null | jq -s '.' || echo '[]')"
IMAGE_JSON="$(cat "$WORK/image.json" 2>/dev/null || echo 'null')"

ARM_RECORD="$(jq -n \
  --arg image "$IMAGE" --arg arm "$ARM" --argjson trials "$TRIAL_JSON" \
  --argjson image_detail "$IMAGE_JSON" --argjson nodes "$NODE_ENVELOPE" \
  --arg measured_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --arg ns "$NAMESPACE" '
  {
    arm: $arm, image: $image, measured_at_utc: $measured_at,
    baked_extensions: ($arm != "before"),
    arm_definition: (if $arm == "before"
      then "same image, ER_DUCKDB_EXTENSION_DIR forced empty so the baked directory is ignored: INSTALL runs, autoinstall is on, extensions are fetched over the network"
      else "the image as shipped: ER_DUCKDB_EXTENSION_DIR points at the baked directory, autoinstall off, no extension egress" end),
    namespace: $ns, image_detail: $image_detail, cluster_nodes: $nodes,
    trials: $trials
  }')"

mkdir -p "$(dirname "$OUT")"
if [ -f "$OUT" ]; then
  BASE="$(cat "$OUT")"
else
  BASE="$(jq -n --arg d "$(date -u +%Y-%m-%d)" '{
    schema_version: 1,
    measured_date_utc: $d,
    scope: "Runner startup: how long a pipeline run takes to begin, split into node launch, image pull and DuckDB extension load.",
    timing_scope: "Per arm. x1/x2 time the probe process itself (container start to DuckLake attached and queried) at millisecond resolution from the process clock. n1/n2 time the Kubernetes lifecycle (NodeClaim created through container started) at the one-second resolution Kubernetes records, except the image pull, which comes from the kubelet Pulled event message and is sub-second.",
    isolation: "One probe at a time on an otherwise idle dev cluster. n1/n2 run on a dedicated measurement NodePool that is deleted afterwards; x1/x2 run on the always-on app pool alongside the namespace web-api Deployment only.",
    limitations: [],
    arms: []
  }')"
fi

printf '%s' "$BASE" \
  | jq --argjson a "$ARM_RECORD" '.arms = ((.arms // []) | map(select(.arm != $a.arm)) + [$a])' \
  > "$WORK/out.json"
mv "$WORK/out.json" "$OUT"
log "wrote $OUT"

# A short human summary, so a run is readable without opening the JSON.
jq -r '
  .arms[] | "arm=\(.arm) image=\(.image)",
  ( .trials[]
    | if .arm == "x1" then "  x1/\(.trial): run1=\(.pod.probe_elapsed_ms // "?")ms run2=\(.pod.probe_second_elapsed_ms // "?")ms fetch=\(.pod.probe_fetch_ms // "?")ms ok=\(.pod.probe_ok // false) pull=\(.pod.pull_seconds // "?")s"
      elif .arm == "x2" then "  x2/\(.trial): job_result=\(.job_result) meets_expectation=\(.meets_expectation)"
      elif .arm == "n1" then "  n1/\(.trial): nodeclaim=\(.nodeclaim.created_at // "?") ready=\(.nodeclaim.ready_at // "?") pull=\(.pod.pull_seconds // "?")s probe=\(.pod.probe_elapsed_ms // "?")ms"
      else "  n2/\(.trial): consolidateAfter=\(.consolidate_after) reused_node=\(.reused_node)"
      end )
' "$OUT" >&2
