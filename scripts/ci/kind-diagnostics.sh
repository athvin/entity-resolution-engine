#!/usr/bin/env bash
# kind.yml's diagnostics-on-failure step: everything needed to read a probe
# failure without re-running it. Only invoked when something already failed,
# so every command tolerates absence (`|| true`) — a diagnostic that fails is
# worse than none. The node budget prints FIRST because "Insufficient cpu" is
# unreadable without it; see the comments inline.
set -uo pipefail

# THE NODE BUDGET FIRST, because "Insufficient cpu" is unreadable
# without it and that is how a whole class of failure hid here. A
# task pod that cannot be scheduled sits `Pending` until the phase's
# wait times out, so the probe reports "job never reached succeeded"
# — a timeout — for a pod that never started. Allocatable CPU and the
# sum of what is already requested are the two numbers that turn that
# back into a one-line diagnosis, and neither was printed.
echo "::group::node capacity and committed requests"
kubectl --context kind-er-kind get nodes \
  -o custom-columns='NODE:.metadata.name,CPU:.status.capacity.cpu,ALLOCATABLE:.status.allocatable.cpu,MEM:.status.allocatable.memory' || true
# `describe node` ends with "Allocated resources" — requests and
# limits as both absolute values and percentages, across every
# namespace. One command, and it is the answer.
kubectl --context kind-er-kind describe node \
  | sed -n '/Allocated resources/,/^Events/p' || true
echo "::endgroup::"
kubectl --context kind-er-kind -n er-kind get pods,jobs -o wide || true
# Why a Pending pod is pending, in its own words. `get pods` shows
# the phase but not the reason, and the reason is the whole point.
for p in $(kubectl --context kind-er-kind -n er-kind get pods \
             --field-selector=status.phase=Pending \
             -o jsonpath='{.items[*].metadata.name}' 2>/dev/null); do
  echo "::group::pending pod $p"
  kubectl --context kind-er-kind -n er-kind describe "pod/$p" \
    | sed -n '/^Containers:/,$p' | head -60 || true
  echo "::endgroup::"
done
kubectl --context kind-er-kind -n er-kind logs deploy/er-dispatcher --tail=200 || true
# By LABEL, not by Deployment name: web-api is two Deployments now
# (one per colour) and `deploy/web-api` no longer resolves. The label
# is stable across both and across a cutover, which is the point — a
# diagnostic that silently prints nothing is worse than none, and this
# one only runs when something has already failed.
kubectl --context kind-er-kind -n er-kind logs -l app.kubernetes.io/name=web-api \
  -c er-server --tail=200 --prefix || true
kubectl --context kind-er-kind -n er-kind get events --sort-by=.lastTimestamp | tail -40 || true
