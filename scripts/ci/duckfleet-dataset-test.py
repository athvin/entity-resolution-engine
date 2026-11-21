"""Rendered namespace/hash and cross-namespace gateway admission contract."""
import hashlib
import subprocess

import yaml

CHART = 'infra/k8s/charts/er-duckfleet-dataset'


def render(org, dataset, client='er'):
    return subprocess.run(['helm', 'template', 'test', CHART,
        '--set-string', f'orgId={org}', '--set-string', f'datasetId={dataset}',
        '--set-string', f'clientNamespace={client}'], capture_output=True, text=True)


namespaces = []
for org, dataset in [('org-a', 'dataset-a'), ('org-a', 'dataset-b'), ('org-b', 'dataset-c')]:
    result = render(org, dataset)
    assert result.returncode == 0, result.stderr
    objects = list(yaml.safe_load_all(result.stdout))
    expected = 'duckfleet-' + hashlib.sha256((org + '\0' + dataset).encode()).hexdigest()[:40]
    namespace = next(o for o in objects if o['kind'] == 'Namespace')
    assert namespace['metadata']['name'] == expected
    namespaces.append(expected)
    policies = {o['metadata']['name']: o['spec'] for o in objects if o['kind'] == 'NetworkPolicy'}
    assert policies['dataset-default-deny'] == {'podSelector': {}, 'policyTypes': ['Ingress', 'Egress']}
    gateway = policies['er-gateway-clients']
    assert gateway['podSelector'] == {'matchLabels': {'app': 'duckfleet-gateway'}}
    assert gateway['ingress'][0]['ports'] == [{'protocol': 'TCP', 'port': 8080}]
    peers = gateway['ingress'][0]['from']
    assert len(peers) == 2
    assert all(p['namespaceSelector']['matchLabels'] == {'kubernetes.io/metadata.name': 'er'} for p in peers)
    assert peers[0]['podSelector']['matchLabels'] == {'app.kubernetes.io/component': 'web-api'}
    assert peers[1]['podSelector']['matchLabels'] == {'er.dupezero.com/duckfleet-dataset': expected}
    quota = next(o for o in objects if o['kind'] == 'ResourceQuota')['spec']['hard']
    assert quota['services.loadbalancers'] == '0' and quota['services.nodeports'] == '0'
assert len(set(namespaces)) == 3
for org, dataset, client in [('', 'a', 'er'), ('org', '../other', 'er'), ('org', 'a', '*')]:
    assert render(org, dataset, client).returncode != 0
print('PASS: dataset namespaces, default deny, scoped HTTP ingress and quota')
