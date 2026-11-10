"""Evaluate the actual bootstrap RBAC against allowed and forbidden requests."""
from pathlib import Path
import yaml

root = Path(__file__).resolve().parents[2]
objects = list(yaml.safe_load_all((root/'infra/k8s/duckfleet/dev/bootstrap.yaml').read_text()))
roles = {(o['kind'], o['metadata']['name']): o for o in objects if o['kind'] in ('Role', 'ClusterRole')}

def allowed(verb, group, resource, namespace=None, name=None):
    for binding in objects:
        if binding['kind'] not in ('RoleBinding', 'ClusterRoleBinding'):
            continue
        if not any(s['kind'] == 'Group' and s['name'] == 'duckfleet:deploy' for s in binding['subjects']):
            continue
        if binding['kind'] == 'RoleBinding' and binding['metadata']['namespace'] != namespace:
            continue
        role = roles[(binding['roleRef']['kind'], binding['roleRef']['name'])]
        for rule in role['rules']:
            if all(wanted in rule[key] or '*' in rule[key] for wanted, key in (
                (verb, 'verbs'), (group, 'apiGroups'), (resource, 'resources')
            )) and ('resourceNames' not in rule or name in rule['resourceNames']):
                return True
    return False

for resource, group in [('deployments','apps'), ('statefulsets','apps'), ('secrets',''), ('jobs','batch')]:
    assert allowed('create', group, resource, 'duckfleet-dev'), resource
    for ns in ['ci', 'er-prod', 'kube-system', 'athvin', None]:
        assert not allowed('create', group, resource, ns), (resource, ns)
        assert not allowed('get', group, resource, ns), (resource, ns)
assert allowed('get', '', 'namespaces', name='duckfleet-dev')
assert allowed('get', 'storage.k8s.io', 'storageclasses', name='gp3')
for resource, group, name in [
    ('namespaces','','ci'), ('nodes','','node'), ('persistentvolumes','','pv'),
    ('storageclasses','storage.k8s.io','other'), ('roles','rbac.authorization.k8s.io','admin'),
    ('rolebindings','rbac.authorization.k8s.io','admin'), ('clusterroles','rbac.authorization.k8s.io','admin'),
    ('clusterrolebindings','rbac.authorization.k8s.io','admin'), ('pods/exec','','worker'),
]:
    for verb in ['get', 'list', 'create', 'patch', 'delete', 'bind', 'escalate', 'impersonate']:
        assert not allowed(verb, group, resource, 'duckfleet-dev', name), (verb, resource)
for resource in ['namespaces', 'resourcequotas']:
    for verb in ['create', 'update', 'patch', 'delete']:
        assert not allowed(verb, '', resource, 'duckfleet-dev', 'duckfleet-dev'), (verb, resource)
namespace = next(o for o in objects if o['kind'] == 'Namespace')
assert namespace['metadata']['labels']['pod-security.kubernetes.io/enforce'] == 'restricted'
quota = next(o for o in objects if o['kind'] == 'ResourceQuota')['spec']['hard']
assert quota['limits.cpu'] == '8' and quota['services.loadbalancers'] == '0'
print('DuckFleet RBAC: application operations allowed; cross-namespace, RBAC, quota and cluster writes denied')
