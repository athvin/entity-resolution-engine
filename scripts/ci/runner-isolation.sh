#!/usr/bin/env bash
# Main-only live acceptance, after Terraform apply and platform deployment.
# This script creates only bounded Jobs/ConfigMaps and reserved fixture objects.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${CLUSTER:?}"
: "${CI_NAMESPACE:?}"
: "${DEPLOY_ENV:?}"
: "${GITHUB_RUN_ID:?}"
: "${GITHUB_RUN_ATTEMPT:?}"
: "${LAKE_BUCKET:?}"
[ "${GITHUB_REF:-}" = refs/heads/main ] || { echo 'main ref required' >&2; exit 2; }
probe_name="er-runner-isolation-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}"
export PROBE_NAME="$probe_name"
kube=(kubectl --context "$CLUSTER" --namespace "$CI_NAMESPACE")
cleanup() {
    "${kube[@]}" delete job "$probe_name-a" "$probe_name-b" --ignore-not-found --wait=false
    "${kube[@]}" delete configmap "$probe_name" --ignore-not-found
}
trap cleanup EXIT
"${kube[@]}" create configmap "$probe_name" \
    --from-file=probe.sh=scripts/ci/runner-isolation-pod.sh --dry-run=client -o json |
    "${kube[@]}" apply -f -
python3 - <<'PY' | "${kube[@]}" apply -f -
import hashlib
import json
import os
from pathlib import Path
registry = json.loads(Path('infra/k8s/charts/er-platform/tenant-runners.json').read_text())
orgs = registry[os.environ['DEPLOY_ENV']][os.environ['CI_NAMESPACE']]
image = 'public.ecr.aws/aws-cli/aws-cli@sha256:eccdb3ec77e159d43ca6a750a70738c546883138be7f094498294a1477ec7e42'
items = []
for suffix, other in [('a', 'b'), ('b', 'a')]:
    org = 'isolation-probe-' + suffix
    own_prefix = orgs[org]
    other_prefix = orgs['isolation-probe-' + other]
    assert own_prefix == '_isolation/probe-' + suffix + '/'
    assert other_prefix == '_isolation/probe-' + other + '/'
    name = os.environ['PROBE_NAME'] + '-' + suffix
    env = {'PROBE_BUCKET': os.environ['LAKE_BUCKET'], 'OWN_PREFIX': own_prefix,
           'OTHER_PREFIX': other_prefix, 'AWS_REGION': 'us-east-2', 'AWS_PAGER': '',
           'AWS_EC2_METADATA_DISABLED': 'true', 'AWS_MAX_ATTEMPTS': '3'}
    items.append({'apiVersion': 'batch/v1', 'kind': 'Job', 'metadata': {'name': name}, 'spec': {
        'backoffLimit': 0, 'activeDeadlineSeconds': 300, 'ttlSecondsAfterFinished': 600,
        'template': {'metadata': {'labels': {'app.kubernetes.io/name': 'er-runner-isolation'}}, 'spec': {
            'restartPolicy': 'Never', 'automountServiceAccountToken': False,
            'serviceAccountName': 'er-runner-' + hashlib.sha256(org.encode()).hexdigest()[:20],
            'nodeSelector': {'karpenter.sh/nodepool': 'app'},
            'securityContext': {'runAsNonRoot': True, 'runAsUser': 1001, 'runAsGroup': 1001,
                                'fsGroup': 1001, 'seccompProfile': {'type': 'RuntimeDefault'}},
            'containers': [{'name': 'probe', 'image': image, 'command': ['/bin/bash', '/probe/probe.sh'],
                'env': [{'name': k, 'value': v} for k, v in env.items()],
                'securityContext': {'allowPrivilegeEscalation': False, 'readOnlyRootFilesystem': True,
                                    'capabilities': {'drop': ['ALL']}},
                'resources': {'requests': {'cpu': '100m', 'memory': '128Mi', 'ephemeral-storage': '100Mi'},
                              'limits': {'cpu': '500m', 'memory': '256Mi', 'ephemeral-storage': '256Mi'}},
                'volumeMounts': [{'name': 'script', 'mountPath': '/probe', 'readOnly': True},
                                 {'name': 'tmp', 'mountPath': '/tmp'}]}],
            'volumes': [{'name': 'script', 'configMap': {'name': os.environ['PROBE_NAME']}},
                        {'name': 'tmp', 'emptyDir': {}}]}}}})
print(json.dumps({'apiVersion': 'v1', 'kind': 'List', 'items': items}))
PY
result=0
"${kube[@]}" wait --for=condition=complete --timeout=330s "job/$probe_name-a" "job/$probe_name-b" || result=$?
"${kube[@]}" logs "job/$probe_name-a" --all-containers=true || result=1
"${kube[@]}" logs "job/$probe_name-b" --all-containers=true || result=1
exit "$result"
