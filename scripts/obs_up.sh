#!/usr/bin/env bash
# Install the cluster's observability stack: log shipper, OTLP collector
# gateway, OTel operator, and the shared span store.
#
# Called by BOTH `make obs-up` and the CI deploy job, for the reason
# `images.yml` already gives about namespace provisioning: "so CI and a human
# provision a namespace identically rather than by two drifting paths". The
# stack used to be Makefile-only, which meant it was hand-installed, in no
# GitOps loop, and invisible to the drift check.
#
# Cluster-scoped and idempotent — every release is `upgrade --install`, so
# running it on every deploy is a no-op when nothing changed.
#
# Chart versions are pinned HERE and nowhere else, so there is exactly one
# place to bump. Config lives in infra/k8s/observability/*.yaml; the AWS side
# (buckets, roles, Pod Identity associations) is Terraform's.
set -euo pipefail

KUBE_CONTEXT="${ER_DEV_CONTEXT:-}"
ctx=()
if [ -n "$KUBE_CONTEXT" ]; then ctx=(--kube-context "$KUBE_CONTEXT"); fi

FLUENT_BIT_VERSION=0.2.0
COLLECTOR_VERSION=0.175.1
OPERATOR_VERSION=0.124.1
OPENOBSERVE_VERSION=1.1.3

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
values="$here/infra/k8s/observability"

kubectl "${ctx[@]}" apply -f "$values/namespace.yaml"

helm repo add eks https://aws.github.io/eks-charts --force-update
helm repo add open-telemetry https://open-telemetry.github.io/opentelemetry-helm-charts --force-update
helm repo add openobserve https://charts.openobserve.ai --force-update

# --- the span store, first --------------------------------------------------
#
# Before the collector, because the collector's traces pipeline exports to it:
# starting the collector first just means it logs connection errors until this
# lands. Credentials come from Secrets Manager (minted by
# envs/dev/observability.tf) rather than from the values file, which is in git —
# the chart offers no existing-secret indirection, so `--set` is the only way
# to keep them out of the repository.
secret="$(aws secretsmanager get-secret-value \
  --secret-id er/shared/openobserve --query SecretString --output text)"
oo_email="$(printf '%s' "$secret" | python3 -c 'import json,sys; print(json.load(sys.stdin)["root_email"])')"
oo_password="$(printf '%s' "$secret" | python3 -c 'import json,sys; print(json.load(sys.stdin)["root_password"])')"
oo_dsn="$(printf '%s' "$secret" | python3 -c 'import json,sys; print(json.load(sys.stdin)["meta_dsn"])')"

helm "${ctx[@]}" upgrade --install openobserve openobserve/openobserve \
  --version "$OPENOBSERVE_VERSION" --namespace observability \
  -f "$values/openobserve-values.yaml" \
  --set "auth.ZO_ROOT_USER_EMAIL=$oo_email" \
  --set "auth.ZO_ROOT_USER_PASSWORD=$oo_password" \
  --set "auth.ZO_META_POSTGRES_DSN=$oo_dsn" \
  --wait --timeout 5m

# The collector and the log shipper authenticate to the store as that root
# user, over HTTP Basic. A dedicated ingest-only user replaces this once the
# store is up and its own user management is reachable; one managed credential
# now beats a second unmanaged one.
oo_basic="$(printf '%s:%s' "$oo_email" "$oo_password" | base64 | tr -d '\n')"
kubectl "${ctx[@]}" -n observability create secret generic openobserve-ingest \
  --from-literal="OPENOBSERVE_AUTH=Basic $oo_basic" \
  --from-literal="OPENOBSERVE_USER=$oo_email" \
  --from-literal="OPENOBSERVE_PASSWORD=$oo_password" \
  --dry-run=client -o yaml | kubectl "${ctx[@]}" apply -f -

# --- shippers ---------------------------------------------------------------

helm "${ctx[@]}" upgrade --install fluent-bit eks/aws-for-fluent-bit \
  --version "$FLUENT_BIT_VERSION" --namespace observability \
  -f "$values/fluent-bit-values.yaml"

helm "${ctx[@]}" upgrade --install otel-collector open-telemetry/opentelemetry-collector \
  --version "$COLLECTOR_VERSION" --namespace observability \
  -f "$values/otel-collector-values.yaml"

helm "${ctx[@]}" upgrade --install otel-operator open-telemetry/opentelemetry-operator \
  --version "$OPERATOR_VERSION" --namespace observability \
  -f "$values/otel-operator-values.yaml"

echo "obs-up: ok (openobserve $OPENOBSERVE_VERSION, collector $COLLECTOR_VERSION)"
