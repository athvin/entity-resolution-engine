#!/usr/bin/env python3
"""Reconcile telemetry only on configuration/secret changes or unhealthy state."""
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
NAMESPACE = 'observability'
MARKER = 'er-observability-applied'


def command(*args):
    return subprocess.check_output(args, text=True)


def healthy(releases, workloads):
    installed = {r['name'] for r in releases if r['status'] == 'deployed'}
    if not {'fluent-bit', 'otel-collector', 'otel-operator'} <= installed:
        return False
    items = workloads['items']
    names = {w['metadata']['name'] for w in items}
    if not {'openobserve', 'otel-collector'} <= names:
        return False
    instances = {w['metadata'].get('labels', {}).get('app.kubernetes.io/instance') for w in items}
    if not {'fluent-bit', 'otel-operator'} <= instances:
        return False
    for item in items:
        spec, status = item['spec'], item.get('status', {})
        if status.get('observedGeneration', 0) < item['metadata']['generation']:
            return False
        if item['kind'] == 'DaemonSet':
            desired = status.get('desiredNumberScheduled', 0)
            ready = min(status.get('numberReady', 0), status.get('updatedNumberScheduled', 0))
        else:
            desired = spec.get('replicas', 1)
            ready = min(status.get('availableReplicas', 0), status.get('updatedReplicas', 0))
        if desired < 1 or ready < desired:
            return False
    return True


def main():
    # Version metadata includes rotations without reading or hashing secret values.
    versions = json.loads(command('aws', 'secretsmanager', 'describe-secret', '--secret-id',
                                  'er/shared/openobserve', '--query', 'VersionIdsToStages', '--output', 'json'))
    current = sorted(v for v, stages in versions.items() if 'AWSCURRENT' in stages)
    if not current:
        raise ValueError('OpenObserve has no current secret version')
    digest = hashlib.sha256(json.dumps(current).encode())
    files = [ROOT/'scripts/obs_up.sh', Path(__file__).resolve(), *sorted((ROOT/'infra/k8s/observability').rglob('*'))]
    for path in files:
        if path.is_file():
            digest.update(str(path.relative_to(ROOT)).encode() + b'\0' + path.read_bytes())
    fingerprint = digest.hexdigest()
    ctx = ['--context', os.environ['ER_DEV_CONTEXT']] if os.environ.get('ER_DEV_CONTEXT') else []
    helm_ctx = ['--kube-context', os.environ['ER_DEV_CONTEXT']] if ctx else []

    def state():
        return healthy(json.loads(command('helm', *helm_ctx, 'list', '-n', NAMESPACE, '--all', '-o', 'json')),
                       json.loads(command('kubectl', *ctx, '-n', NAMESPACE, 'get', 'deployment,daemonset', '-o', 'json')))

    try:
        marker = json.loads(command('kubectl', *ctx, '-n', NAMESPACE, 'get', 'configmap', MARKER, '-o', 'json'))
        if marker['data']['fingerprint'] == fingerprint and state():
            print('observability: unchanged and healthy; skipping reconciliation')
            return
    except (subprocess.CalledProcessError, KeyError, ValueError):
        pass  # Bootstrap, missing marker or unreadable state requires reconciliation.
    subprocess.run(['bash', str(ROOT/'scripts/obs_up.sh')], check=True)
    workloads = json.loads(command('kubectl', *ctx, '-n', NAMESPACE, 'get', 'deployment,daemonset', '-o', 'json'))
    for item in workloads['items']:
        resource = item['kind'].lower() + '/' + item['metadata']['name']
        subprocess.run(['kubectl', *ctx, '-n', NAMESPACE, 'rollout', 'status', resource, '--timeout=3m'], check=True)
    if not state():
        raise RuntimeError('observability is not fully rolled out; refusing to record success')
    manifest = {'apiVersion': 'v1', 'kind': 'ConfigMap', 'metadata': {'name': MARKER, 'namespace': NAMESPACE},
                'data': {'fingerprint': fingerprint}}
    subprocess.run(['kubectl', *ctx, 'apply', '-f', '-'], input=json.dumps(manifest), text=True, check=True)
    print('observability: reconciled and verified')


if __name__ == '__main__':
    main()
