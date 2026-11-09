#!/usr/bin/env python3
"""Prove the live probe rejects allowed foreign access and credential failures."""
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
if mode == 'no-credentials':
    print('Unable to locate credentials', file=sys.stderr)
    sys.exit(1)
if not own:
    if mode == label:
        sys.exit(0)
    error = 'EndpointConnectionError' if mode == 'network-error' else '(AccessDenied)'
    print(error, file=sys.stderr)
    sys.exit(1)
if action == 'get-object':
    pathlib.Path(args[-1]).write_text('tenant-isolation-positive-control')
'''

with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    cli = root / 'aws-stub'
    cli.write_text('#!' + sys.executable + '\n' + STUB)
    cli.chmod(0o755)
    def run(mode, own='_isolation/probe-a/', other='_isolation/probe-b/'):
        calls = root / 'calls'
        calls.write_text('')
        env = dict(os.environ, AWS_CLI=str(cli), STUB_MODE=mode, CALL_LOG=str(calls),
                   PROBE_BUCKET='fixture', OWN_PREFIX=own, OTHER_PREFIX=other, TMPDIR=str(root))
        result = subprocess.run(['bash', str(PROBE)], env=env, text=True, capture_output=True)
        return result, calls.read_text()

    for own, other in [('_isolation/probe-a/', '_isolation/probe-b/'),
                       ('_isolation/probe-b/', '_isolation/probe-a/')]:
        result, calls = run('correct-policy', own, other)
        assert result.returncode == 0, result.stderr
        assert 'PASS: tenant S3 isolation' in result.stdout
        assert calls.count('get-object:True') == 1 and calls.count('put-object:True') == 1
        assert calls.endswith('delete-object:True\n'), calls
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
