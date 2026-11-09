#!/usr/bin/env bash
# Apply the cluster-scoped objects to one environment's cluster, then REPORT
# what the cluster actually holds. `make cluster-objects` and the
# cluster-objects.yml workflow both call this — one code path, so the human
# command and CI cannot drift apart (the obs_up.sh precedent).
#
# The objects: the Karpenter NodePools, the task PriorityClasses
# (batch-orchestration-design §7.1 — cluster-scoped, so no per-namespace chart
# can carry them), and the CNI setting that makes NetworkPolicy enforce at
# all. They are Kubernetes manifests, not AWS API objects, so Terraform does
# not own them (infra/terraform/modules/eks/main.tf says so) and no chart
# renders them. Before this set was applied from CI, a cluster was BORN
# without them and nothing reported it — er-prod measured nine days with no
# app/runner NodePools, no er-task-* PriorityClass and no amazon-vpc-cni
# ConfigMap, so every NetworkPolicy in it enforced nothing (§8.5's silent
# failure, silent precisely because the policies exist and read as healthy).
#
# THREE APPLY SURFACES FOR NODEPOOLS, AND THE SPLIT IS LOAD-BEARING.
# `kubectl apply -f <dir>` is non-recursive, so the top level carries the
# pools both clusters share (runner-sm, runner-l — §5 gives them the same
# policy in either environment) and `<env>/` carries the ones that differ —
# today exactly `app` ("spot in dev, on-demand in prod", and a pool cannot
# express both). Both files declare a NodePool NAMED `app`, fine because they
# are applied to different clusters and the chart pins no karpenter.sh/
# nodepool. infra/k8s/nodepools/measure/ is deliberately NOT applied: the
# throwaway pool scripts/measure_runner_startup.sh creates and deletes lives
# there, excluded by never being named rather than by an exclusion list.
#
# AFTER APPLYING, VERIFY AND REPORT — the point of the ticket (#927):
#   * presence: `kubectl get -f` on every applied surface. An object the
#     apply just wrote must be gettable; a missing one fails the run loudly
#     instead of converging silently past it.
#   * convergence: `kubectl diff -f` post-apply should be empty. A persistent
#     diff means the server would not take what the tree says (an immutable
#     field fight, a defaulting webhook arguing back). Reported as a notice,
#     not a failure: server-side defaulting can make a benign diff permanent,
#     and a red run nobody can act on teaches readers to ignore this one.
#   * deletion drift: objects on the cluster whose names the tree no longer
#     declares (a pool removed from git still runs). Reported as a notice,
#     never pruned — `kubectl apply` converges but never deletes, and deleting
#     capacity from CI is the dangerous direction. The throwaway measure/ pool
#     is expected in this list while a measurement runs.
#
# Usage: scripts/cluster_objects.sh <env> [context]   (context defaults er-<env>)
set -euo pipefail

cd "$(dirname "$0")/.."

ENV="${1:?usage: cluster_objects.sh <env> [context]}"
CONTEXT="${2:-er-$ENV}"

case "$ENV" in
dev | prod) ;;
*)
	echo "::error::env must be dev or prod, got '$ENV'"
	exit 1
	;;
esac

# External Secrets Operator FIRST, and pinned. Two failures shaped this:
#
#   * ESO was installed into er-dev by hand in task 1.6 (2026-10-10) and never
#     captured in any script — the same unowned-cluster-object class this
#     whole script exists to close. er-prod measured the cost on its first
#     deploy (#556): the release's pre-install hooks reference ExternalSecret
#     objects, the CRDs were absent, and helm failed the install with "no
#     matches for kind ExternalSecret". The Terraform (envs/*/secrets.tf)
#     covers only the IAM half — the role and the Pod Identity association —
#     never the controller.
#   * The version MUST be pinned: an unpinned `upgrade --install` silently
#     tracks latest, so the two clusters would run different ESO versions
#     depending on when each last applied. 2.12.0 is what er-dev has run since
#     the hand install; bump it here and both clusters move together.
#
# `--wait`: the ClusterSecretStore below cannot validate against a controller
# that is not up, and the deploy's hooks cannot build ExternalSecret objects
# until the CRDs exist.
ESO_CHART_VERSION=2.12.0
helm repo add external-secrets https://charts.external-secrets.io >/dev/null 2>&1 || true
helm --kube-context "$CONTEXT" upgrade --install external-secrets \
	external-secrets/external-secrets \
	--version "$ESO_CHART_VERSION" \
	--namespace external-secrets --create-namespace --wait --timeout 5m

SURFACES=(
	"infra/k8s/nodepools/"
	"infra/k8s/nodepools/$ENV/"
	"infra/k8s/priorityclasses/"
	"infra/k8s/network-policy-controller.yaml"
	# The one ClusterSecretStore every namespace's ExternalSecrets reference
	# (§9). Environment-agnostic: authentication is the controller's Pod
	# Identity, so one file serves both clusters while the IAM scoping
	# differs (dev reads er/*, prod er/prod/*). Ordering is the ESO install
	# above — the store's kind does not exist until the CRDs land.
	"infra/k8s/secrets/cluster-secret-store.yaml"
)

for surface in "${SURFACES[@]}"; do
	kubectl --context "$CONTEXT" apply -f "$surface"
done

echo "--- verifying presence (kubectl get -f on every applied surface) ---"
for surface in "${SURFACES[@]}"; do
	kubectl --context "$CONTEXT" get -f "$surface" >/dev/null
done
echo "presence: every applied object is gettable"

echo "--- convergence (kubectl diff -f, post-apply; should be silent) ---"
for surface in "${SURFACES[@]}"; do
	if ! diff_out="$(kubectl --context "$CONTEXT" diff -f "$surface" 2>&1)"; then
		echo "$diff_out"
		echo "::notice title=$CONTEXT $surface::post-apply diff not empty — the cluster would not take what the tree says; if this persists across runs it is real drift, not defaulting"
	fi
done

echo "--- deletion drift (on the cluster, not in the tree; report only) ---"
# The names the tree declares, from the same files just applied.
tree_pools="$(kubectl --context "$CONTEXT" apply --dry-run=client -f infra/k8s/nodepools/ -f "infra/k8s/nodepools/$ENV/" -o name 2>/dev/null | sed 's|.*/||' | sort -u)"
live_pools="$(kubectl --context "$CONTEXT" get nodepools -o name | sed 's|.*/||' | sort -u)"
for pool in $live_pools; do
	# `system` is Auto Mode's own built-in pool (modules/eks sets
	# node_pools = ["system"]) — a permanent resident the tree will never
	# declare, so reporting it every run would be pure noise.
	[ "$pool" = "system" ] && continue
	# grep -qx, an exact-LINE match — the lists are newline-separated, and a
	# glob over " $list " only matches space-separated entries (it reported
	# every pool as drift in the smoke test).
	echo "$tree_pools" | grep -qx "$pool" ||
		echo "::notice title=$CONTEXT nodepool/$pool::exists on the cluster but not in the tree — apply never deletes; remove by hand if it is really retired (measure/ pools are expected here while a measurement runs)"
done
tree_pcs="$(kubectl --context "$CONTEXT" apply --dry-run=client -f infra/k8s/priorityclasses/ -o name 2>/dev/null | sed 's|.*/||' | sort -u)"
live_pcs="$(kubectl --context "$CONTEXT" get priorityclasses -o name | sed 's|.*/||' | grep '^er-task-' | sort -u)"
for pc in $live_pcs; do
	echo "$tree_pcs" | grep -qx "$pc" ||
		echo "::notice title=$CONTEXT priorityclass/$pc::exists on the cluster but not in the tree — remove by hand if retired"
done

# COMMA, not a space. `kubectl get nodepools priorityclasses` reads the second
# word as a resource NAME and fails with `nodepools.karpenter.sh
# "priorityclasses" not found` — the shape that made this target exit 1 on
# every invocation before #906, after applying everything successfully,
# unnoticed because the applies print success first and the error arrives last.
kubectl --context "$CONTEXT" get nodepools,priorityclasses
