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

# `${ctx[@]+"${ctx[@]}"}` rather than `"${ctx[@]}"`, which looks like noise and
# is not. macOS ships **bash 3.2**, where expanding an EMPTY array under
# `set -u` is an "unbound variable" error; bash 4.4+ (every CI runner) treats it
# as nothing. So the plain form worked in CI and failed on line 1 for anyone
# running `make obs-up` on a Mac — which is the entire human half of the "one
# script, two callers" promise this file opens with. It went unnoticed because
# the CI deploy job was being cancelled by images.yml's concurrency before it
# ran, so this script had never successfully executed anywhere.

FLUENT_BIT_VERSION=0.2.0
COLLECTOR_VERSION=0.175.1
OPERATOR_VERSION=0.124.1

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
values="$here/infra/k8s/observability"

kubectl ${ctx[@]+"${ctx[@]}"} apply -f "$values/namespace.yaml"

helm repo add eks https://aws.github.io/eks-charts --force-update
helm repo add open-telemetry https://open-telemetry.github.io/opentelemetry-helm-charts --force-update

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

# The store is a plain Deployment, not a chart — openobserve.yaml's header
# carries the measurement that decided it (the distributed chart is ~10 pods
# plus NATS plus a bundled Postgres, on a 4-vCPU cluster).
kubectl ${ctx[@]+"${ctx[@]}"} -n observability create secret generic openobserve-env \
  --from-literal="ZO_ROOT_USER_EMAIL=$oo_email" \
  --from-literal="ZO_ROOT_USER_PASSWORD=$oo_password" \
  --from-literal="ZO_META_POSTGRES_DSN=$oo_dsn" \
  --dry-run=client -o yaml | kubectl ${ctx[@]+"${ctx[@]}"} apply -f -

kubectl ${ctx[@]+"${ctx[@]}"} apply -f "$values/openobserve.yaml"
kubectl ${ctx[@]+"${ctx[@]}"} -n observability rollout status deploy/openobserve --timeout=5m

# The collector and the log shipper authenticate to the store as that root
# user, over HTTP Basic. A dedicated ingest-only user replaces this once the
# store is up and its own user management is reachable; one managed credential
# now beats a second unmanaged one.
oo_basic="$(printf '%s:%s' "$oo_email" "$oo_password" | base64 | tr -d '\n')"
kubectl ${ctx[@]+"${ctx[@]}"} -n observability create secret generic openobserve-ingest \
  --from-literal="OPENOBSERVE_AUTH=Basic $oo_basic" \
  --from-literal="OPENOBSERVE_USER=$oo_email" \
  --from-literal="OPENOBSERVE_PASSWORD=$oo_password" \
  --dry-run=client -o yaml | kubectl ${ctx[@]+"${ctx[@]}"} apply -f -

# --- shippers ---------------------------------------------------------------

helm ${ctx[@]+"${ctx[@]}"} upgrade --install fluent-bit eks/aws-for-fluent-bit \
  --version "$FLUENT_BIT_VERSION" --namespace observability \
  -f "$values/fluent-bit-values.yaml"

helm ${ctx[@]+"${ctx[@]}"} upgrade --install otel-collector open-telemetry/opentelemetry-collector \
  --version "$COLLECTOR_VERSION" --namespace observability \
  -f "$values/otel-collector-values.yaml"

helm ${ctx[@]+"${ctx[@]}"} upgrade --install otel-operator open-telemetry/opentelemetry-operator \
  --version "$OPERATOR_VERSION" --namespace observability \
  -f "$values/otel-operator-values.yaml"

# Wait for the collector to actually come up, which `helm upgrade --install`
# does NOT do: it reports success once the objects are applied.
#
# This is not hypothetical. A `#` comment inside the `transform/pii_allowlist`
# OTTL expression made the collector refuse its whole config, and the deploy
# reported SUCCESS: the ConfigMap updated, the Deployment rolled, the new pod
# crashlooped, and the OLD pod kept serving. So telemetry kept flowing, the run
# was green, and the change had simply not taken effect. Only reading pod health
# showed it.
#
# `deploy.yml` already states the intent — "a telemetry stack that silently
# fails to install is worse than a red deploy" — and this is what delivers it.
# Only the collector is waited on: it is the one release here that takes a
# config it can reject at startup, and openobserve already has its own wait
# above.
kubectl ${ctx[@]+"${ctx[@]}"} -n observability rollout status deploy/otel-collector --timeout=3m

echo "obs-up: ok (openobserve v1.1.2 Deployment, collector $COLLECTOR_VERSION)"
