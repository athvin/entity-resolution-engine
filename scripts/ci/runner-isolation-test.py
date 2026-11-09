#!/usr/bin/env python3
"""Prove the live probe rejects allowed foreign access and credential failures."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

PROBE = Path(__file__).with_name("runner-isolation-pod.sh").resolve()
STUB = r'''
import os, pathlib, sys
args = sys.argv[1:]
action = args[1]
key = args[args.index('--key') + 1] if '--key' in args else None
prefix = args[args.index('--prefix') + 1] if '--prefix' in args else None
own = (key or prefix or '').startswith(os.environ['OWN_PREFIX'])
label = 'bucket-root-list' if key is None and prefix is None else 'cross-prefix-' + {
    'put-object': 'put', 'get-object': 'get', 'list-objects-v2': 'list', 'delete-object': 'delete'
}[action]
with open(os.environ['CALL_LOG'], 'a') as log:
    log.write(action + ':' + str(own) + '\n')
mode = os.environ['STUB_MODE']
store = pathlib.Path(os.environ['STUB_STORE'])
object_file = store / key if key else None
if mode == 'no-credentials':
    print('Unable to locate credentials', file=sys.stderr)
    sys.exit(1)
if not own:
    if mode == label:
        # S3 can conceal a missing key behind AccessDenied without ListBucket.
        if action == 'get-object' and not object_file.exists():
            print('(AccessDenied)', file=sys.stderr)
            sys.exit(1)
        sys.exit(0)
    error = 'EndpointConnectionError' if mode == 'network-error' else '(AccessDenied)'
    print(error, file=sys.stderr)
    sys.exit(1)
if action == 'put-object':
    object_file.parent.mkdir(parents=True, exist_ok=True)
    object_file.write_bytes(pathlib.Path(args[args.index('--body') + 1]).read_bytes())
if action == 'get-object':
    pathlib.Path(args[-1]).write_bytes(object_file.read_bytes())
'''

with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    cli = root / 'aws-stub'
    cli.write_text('#!' + sys.executable + '\n' + STUB)
    cli.chmod(0o755)
    def run(mode, own='_isolation/probe-a/', other='_isolation/probe-b/', phase='verify'):
        calls = root / 'calls'
        calls.write_text('')
        env = dict(os.environ, AWS_CLI=str(cli), STUB_MODE=mode, CALL_LOG=str(calls),
                   PROBE_BUCKET='fixture', OWN_PREFIX=own, OTHER_PREFIX=other, TMPDIR=str(root),
                   PROBE_PHASE=phase, STUB_STORE=str(root / 'objects'))
        result = subprocess.run(['bash', str(PROBE)], env=env, text=True, capture_output=True)
        return result, calls.read_text()

    # The orchestrator prepares BOTH fixtures before either verify phase.
    for own, other in [('_isolation/probe-a/', '_isolation/probe-b/'),
                       ('_isolation/probe-b/', '_isolation/probe-a/')]:
        result, calls = run('correct-policy', own, other, phase='prepare')
        assert result.returncode == 0 and 'fixture marker prepared' in result.stdout, result
        assert ':False' not in calls, calls
    for own, other in [('_isolation/probe-a/', '_isolation/probe-b/'),
                       ('_isolation/probe-b/', '_isolation/probe-a/')]:
        result, calls = run('correct-policy', own, other)
        assert result.returncode == 0, result.stderr
        assert 'PASS: tenant S3 isolation' in result.stdout
        assert calls.count('get-object:True') == 1 and calls.count('put-object:True') == 1
        assert 'delete-object:True' not in calls, calls
        assert (root / 'objects' / other / 'boundary-marker').exists()
    result, calls = run('correct-policy', phase='unknown')
    assert result.returncode == 2 and not calls, result
    for operation in ['put', 'get', 'list', 'delete']:
        label = 'cross-prefix-' + operation
        result, _ = run(label)
        assert result.returncode != 0 and f'{label} was allowed' in result.stderr, result
    result, _ = run('bucket-root-list')
    assert result.returncode != 0 and 'bucket-root-list was allowed' in result.stderr, result
    result, _ = run('network-error')
    assert result.returncode != 0 and 'did not return AccessDenied' in result.stderr, result
    result, _ = run('no-credentials')
    assert result.returncode != 0 and 'PASS:' not in result.stdout, result
    result, calls = run('correct-policy', 'customer/', '_isolation/probe-b/')
    assert result.returncode == 2 and not calls, result
print('isolation probe: positive controls, five permission leaks, network/credential failure and prefix guard pass')

# Exercise the live-input mode with actual chart output, then stale/unsafe objects.
# CI installs Helm and PyYAML before this script; the live workflow needs neither.
import yaml

rendered = subprocess.check_output([
    "helm", "template", "er-platform", "infra/k8s/charts/er-platform", "--namespace", "ci",
    "--set", "environment=dev", "--set", "efs.fileSystemId=fs-0", "--set", "efs.accessPointId=fsap-0",
], text=True)
docs = [d for d in yaml.safe_load_all(rendered) if d]
with tempfile.TemporaryDirectory() as directory:
    state = Path(directory) / "live.json"
    def check_live(items):
        state.write_text(json.dumps({"items": items}))
        return subprocess.run([
            sys.executable, "-S", str(PROBE.with_name("tenant-runners-check.py")),
            "--live-json", str(state), "--environment", "dev", "--namespace", "ci",
        ], text=True, capture_output=True)
    result = check_live(docs)
    assert result.returncode == 0, result.stderr
    stale = copy.deepcopy(docs)
    dispatcher = next(d for d in stale if d.get("kind") == "Deployment" and d["metadata"]["name"] == "er-dispatcher")
    env = dispatcher["spec"]["template"]["spec"]["containers"][0]["env"]
    next(v for v in env if v["name"] == "ER_TENANT_RUNNER_ORGS")["value"] = "[]"
    result = check_live(stale)
    assert result.returncode != 0 and "enrollment disagrees" in result.stderr, result
    account = next(d["metadata"]["name"] for d in docs if d.get("kind") == "ServiceAccount" and d["metadata"]["name"].startswith("er-runner-"))
    missing = [d for d in docs if not (d.get("kind") == "ServiceAccount" and d["metadata"]["name"] == account)]
    result = check_live(missing)
    assert result.returncode != 0 and "accounts disagree" in result.stderr, result
    bound = docs + [{"kind": "RoleBinding", "subjects": [{"kind": "ServiceAccount", "name": account}]}]
    result = check_live(bound)
    assert result.returncode != 0 and "runner has Kubernetes RBAC" in result.stderr, result
print("live enrollment: rendered objects pass; stale dispatcher, missing account and runner RBAC are refused")
