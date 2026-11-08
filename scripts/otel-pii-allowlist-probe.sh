#!/usr/bin/env bash
# otel-pii-allowlist-probe.sh — BEHAVIOURAL probe for the collector's
# transform/pii_allowlist processor: send one OTLP span carrying a deliberately
# non-allowlisted attribute (`er.probe_leak`) plus an allowlisted control
# (`er.action`), and assert the export keeps the control and drops the probe.
#
# The four tests in er-schema/tests/collector_allowlist.rs all READ the config;
# this script OBSERVES it. It runs the collector's live ConfigMap (or a file
# passed as $1, e.g. a `helm template` render for a pre-deploy check) in a
# local Docker container of the pinned contrib image, with only the exporters
# swapped for `debug` — the processor chain is semantically identical to
# what runs on er-dev (the edit re-serializes the YAML, but the parsed config
# the collector consumes is unchanged).
#
# Why the control matters: without `er.action` surviving, a span that never
# arrived looks identical to a span whose attribute was dropped. `er.action`
# also matches the ui-interactions tail-sampling policy, so the probe trace is
# kept deterministically rather than falling to the 10% residual.
#
# Usage:
#   scripts/otel-pii-allowlist-probe.sh [config-file]
#   KUBECTL_CONTEXT=er-dev-ro scripts/otel-pii-allowlist-probe.sh   # default
#
# [config-file] is the collector config YAML itself — the content of the
# ConfigMap's `relay` key, not a raw helm stream. To probe a render before
# deploying it:
#   helm template otel-collector open-telemetry/opentelemetry-collector \
#     --version 0.175.1 -f infra/k8s/observability/otel-collector-values.yaml \
#     --show-only templates/configmap.yaml \
#   | python3 -c 'import sys, yaml; print(yaml.safe_load(sys.stdin)["data"]["relay"])' \
#     > /tmp/rendered-collector.yaml
#   scripts/otel-pii-allowlist-probe.sh /tmp/rendered-collector.yaml
#
# Exit 0 = PASS (control present, probe absent). Exit 1 = FAIL, with the
# collector log dumped. Re-run after any bump of the collector image or the
# tracing-opentelemetry pins (the SDK-added attributes on the allowlist are
# found by measurement, per collector_allowlist.rs's doc comment).
set -euo pipefail

python3 -c 'import yaml' 2>/dev/null || {
  echo "needs python3 with PyYAML (pip3 install pyyaml)" >&2
  exit 1
}

# The probe needs only a ConfigMap read, so default to the least-privilege
# context; ER_DEV_CONTEXT (the Makefile's var) and KUBECTL_CONTEXT both override.
KUBECTL_CONTEXT="${KUBECTL_CONTEXT:-${ER_DEV_CONTEXT:-er-dev-ro}}"
IMAGE_TAG="${IMAGE_TAG:-$(sed -n 's/^  tag: \([0-9.]*\).*pinned to the chart.*/\1/p' infra/k8s/observability/otel-collector-values.yaml)}"
IMAGE="otel/opentelemetry-collector-contrib:${IMAGE_TAG:?could not parse the pinned tag from the values file}"
CONTAINER="er-pii-probe-$$"
TRACE_ID="aa668aa668aa668aa668aa668aa668aa"   # fixed, grep-able
PROBE_VALUE="alice@example.com"

workdir="$(mktemp -d)"
trap 'docker rm -f "$CONTAINER" >/dev/null 2>&1 || true; rm -rf "$workdir"' EXIT

# 1. The config under test: an explicit file, else the live ConfigMap.
if [ "${1:-}" ]; then
  cp "$1" "$workdir/live.yaml"
else
  kubectl --context "$KUBECTL_CONTEXT" -n observability \
    get configmap otel-collector -o jsonpath='{.data.relay}' > "$workdir/live.yaml"
fi

# 2. Swap ONLY the exporters for debug; processors stay byte-identical.
python3 - "$workdir/live.yaml" "$workdir/probe.yaml" << 'PY'
import sys
import yaml

live_path, out_path = sys.argv[1], sys.argv[2]
config = yaml.safe_load(open(live_path))
config["exporters"] = {"debug": {"verbosity": "detailed"}}
traces = config["service"]["pipelines"]["traces"]
config["service"]["pipelines"] = {
    "traces": {**traces, "exporters": ["debug"]},
}
yaml.safe_dump(config, open(out_path, "w"), default_flow_style=False)
PY

# 3. Run the pinned collector image. MY_POD_IP=0.0.0.0 because the OTLP
#    receiver's endpoint is ${env:MY_POD_IP}:4318 in the live config.
docker run -d --name "$CONTAINER" \
  -e MY_POD_IP=0.0.0.0 \
  -p 127.0.0.1:4318:4318 \
  -v "$workdir/probe.yaml:/etc/otelcol-contrib/config.yaml:ro" \
  "$IMAGE" >/dev/null

# `grep -c`, never `grep -q` in a pipe: -q closes the pipe on first match,
# SIGPIPEs `docker logs`, and the pipeline exits 141 under pipefail — a found
# match reporting as missing, intermittently (detect-paths.sh documents the
# trap; the repo has been bitten twice).
ready=0
for _ in $(seq 1 30); do
  docker logs "$CONTAINER" > "$workdir/boot.log" 2>&1 || true
  if [ "$(grep -c "Everything is ready" "$workdir/boot.log" || true)" -gt 0 ]; then
    ready=1
    break
  fi
  sleep 1
done
if [ "$ready" -ne 1 ]; then
  echo "FAIL: collector never became ready — config rejected?" >&2
  tail -20 "$workdir/boot.log" >&2
  exit 1
fi

# 4. One span: control + probe attribute.
curl -sf -X POST http://127.0.0.1:4318/v1/traces \
  -H 'Content-Type: application/json' \
  -d @- << JSON > /dev/null
{
  "resourceSpans": [{
    "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "pii-allowlist-probe"}}]},
    "scopeSpans": [{
      "scope": {"name": "probe"},
      "spans": [{
        "traceId": "$TRACE_ID",
        "spanId": "bb668bb668bb668b",
        "name": "pii-allowlist-probe",
        "kind": 1,
        "startTimeUnixNano": "1700000000000000000",
        "endTimeUnixNano": "1700000001000000000",
        "attributes": [
          {"key": "er.action", "value": {"stringValue": "pii_allowlist_probe"}},
          {"key": "er.probe_leak", "value": {"stringValue": "$PROBE_VALUE"}}
        ]
      }]
    }]
  }]
}
JSON

# 5. tail_sampling holds the trace for decision_wait (10s); batch adds a beat.
sleep 15
docker logs "$CONTAINER" > "$workdir/out.log" 2>&1

control=$(grep -c "pii_allowlist_probe" "$workdir/out.log" || true)
leak=$(grep -c -e "er\.probe_leak" -e "$PROBE_VALUE" "$workdir/out.log" || true)

if [ "$control" -gt 0 ] && [ "$leak" -eq 0 ]; then
  echo "PASS: control attribute er.action exported ($control sighting(s)); er.probe_leak and its value never left the collector."
  exit 0
fi
echo "FAIL: control sightings=$control (want >0), leak sightings=$leak (want 0)" >&2
grep -A30 "$TRACE_ID" "$workdir/out.log" | head -50 >&2
exit 1
