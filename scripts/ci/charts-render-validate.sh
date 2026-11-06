#!/usr/bin/env bash
# infra.yml's chart job: render every profile and schema-validate it.
#
# Three renders, because they are three different manifest sets and a template
# guarded by `if` is unchecked in the profiles that disable it: the default
# dev profile, the kind profile (`web.enabled: false`, so no BFF container and
# no drizzle hook), and the namespace chart.
#
# `-ignore-missing-schemas` is needed for the ExternalSecret CRD, which has no
# published schema. It is a real gap — a typo in a CRD's `kind` would be
# skipped rather than caught — which is why this prints the skipped count
# rather than hiding it, and why the selector gate (charts-check.py) checks
# relationships no schema could express anyway.
set -euo pipefail
render() {
  echo "== $1 =="
  shift
  helm template "$@" > /tmp/render.yaml
  kubeconform -strict -summary -ignore-missing-schemas \
    -kubernetes-version 1.32.0 /tmp/render.yaml
}
render "er-platform (dev profile)" er-platform infra/k8s/charts/er-platform \
  --namespace ci --set developer=ci \
  --set efs.fileSystemId=fs-0 --set efs.accessPointId=fsap-0 \
  --set ingress.enabled=true \
  --set 'ingress.certificateArn=arn:aws:acm:us-east-2:0:certificate/0'
render "er-platform (kind profile)" er-platform infra/k8s/charts/er-platform \
  --namespace er -f infra/k8s/charts/er-platform/values-kind.yaml
render "er-dev-namespace" ci infra/k8s/charts/er-dev-namespace \
  --namespace default --set developer=ci \
  --set webIngressFromVpc.enabled=true
