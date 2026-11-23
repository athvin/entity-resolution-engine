"""Disposable three-deployment isolation proof; no existing fleet is contacted.

This proves HTTP credentials, catalog/session/history separation in Compose.
It does not establish EKS NetworkPolicy or AWS IAM enforcement; qualification
must run the corresponding cloud probe before enrolling customer datasets.
"""
import argparse
import contextlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import time
import urllib.request
import uuid

from compatibility import Remote, RemoteError


def refused(action, statuses):
    try:
        action()
    except RemoteError as error:
        assert error.status in statuses, f'unexpected refusal status {error.status}'
        return
    raise AssertionError('cross-dataset operation unexpectedly succeeded')


def run(source, output, image=None):
    project_root = 'er-duckfleet-isolation-' + uuid.uuid4().hex[:12]
    projects = []
    evidence = {'upstream_revision': subprocess.check_output(
        ['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip(),
        'scope': 'disposable Compose; not live AWS qualification', 'checks': []}
    if image:
        evidence['image_id'] = subprocess.check_output(['docker', 'image', 'inspect', image, '--format', '{{.Id}}'], text=True).strip()
    with tempfile.TemporaryDirectory(prefix=project_root) as scratch:
        scratch = Path(scratch)
        with (scratch / 'compose.log').open('w') as log:
            try:
                for index in range(3):
                    project = f'{project_root}-{index}'
                    token = secrets.token_hex(32)
                    password = secrets.token_hex(32)
                    # Upstream provisions per-component roles from the administrator
                    # password; the role DSNs come from its compose file.
                    # Tokens go through upstream's compose variables (see env below):
                    # an ordinary worker refuses the public token.
                    override = {'services': {
                        'worker-small': {'image': image or project_root + ':isolation', 'environment': {
                            'DUCKFLEET_HISTORY_RETENTION_DAYS': '1'}},
                        'server': {'image': image or project_root + ':isolation', 'pull_policy': 'never'}}}
                    config = scratch / f'{index}.json'
                    config.write_text(json.dumps(override))
                    config.chmod(0o600)
                    cmd = ['docker', 'compose', '--project-name', project,
                        '--file', str(source / 'compose.yaml'), '--file', str(config)]
                    env = dict(os.environ, DUCKFLEET_PORT='0', DUCKFLEET_POSTGRES_ADMIN_PASSWORD=password,
                        DUCKFLEET_TOKEN=token, DUCKFLEET_INTERNAL_TOKEN=secrets.token_hex(32),
                        DUCKFLEET_WORKER_URLS='http://worker-small:8080')
                    projects.append((cmd, env))
                    subprocess.run(cmd + ['up', '--detach'] + ([] if image else ['--build']) + ['server'],
                        env=env, stdout=log, stderr=log, check=True)
                    address = subprocess.check_output(cmd + ['port', 'server', '8080'],
                        env=env, text=True).strip()
                    remote = Remote('http://' + address, token)
                    for attempt in range(120):
                        try:
                            urllib.request.urlopen(remote.url + '/healthz', timeout=2).close()
                            break
                        except OSError:
                            time.sleep(1)
                    else:
                        raise AssertionError('disposable gateway did not become healthy')
                    projects[-1] = (cmd, env, remote)
                remotes = [p[2] for p in projects]
                with contextlib.ExitStack() as stack:
                    for i, remote in enumerate(remotes):
                        stack.enter_context(remote)
                        remote.query('CREATE TABLE sentinel (dataset INTEGER)')
                        remote.query('INSERT INTO sentinel VALUES (?)', [i])
                        assert remote.query('SELECT dataset FROM sentinel').to_pylist() == [{'dataset': i}]
                        remote.query("COPY sentinel TO 's3://duckfleet/artifacts/sentinel.parquet' (FORMAT PARQUET)")
                        assert remote.query("SELECT dataset FROM read_parquet('s3://duckfleet/artifacts/sentinel.parquet')").to_pylist() == [{'dataset': i}]
                    for i, remote in enumerate(remotes):
                        history = json.loads(remote.request('GET', '/v1/queries'))['queries']
                        own_query = next(q['query_id'] for q in history if q['session_id'] == remote.session)
                        for j, other in enumerate(remotes):
                            if i == j:
                                continue
                            foreign = Remote(other.url, remote.token)
                            refused(lambda: foreign.request('POST', '/v1/sessions', {}), {401, 403})
                            refused(lambda: foreign.request('GET', '/v1/queries'), {401, 403})
                            refused(lambda: foreign.request('POST', '/v1/sessions/' + other.session + '/queries',
                                {'request_id': str(uuid.uuid4()), 'sql': 'DELETE FROM sentinel',
                                 'parameters': [], 'timeout_seconds': 5}), {401, 403})
                            refused(lambda: other.request('GET', '/v1/queries/' + own_query), {404})
                            refused(lambda: other.request('POST', '/v1/sessions/' + remote.session + '/queries',
                                {'request_id': str(uuid.uuid4()), 'sql': 'SELECT * FROM sentinel',
                                 'parameters': [], 'timeout_seconds': 5}), {404})
                            refused(lambda: other.request('DELETE', '/v1/sessions/' + remote.session), {404})
                            evidence['checks'].append({'from_dataset': i, 'to_dataset': j,
                                'foreign_credential': 'refused', 'foreign_write': 'refused', 'foreign_history': 'refused',
                                'foreign_session_read_delete': 'refused'})
                        # Credential strings must not appear in persisted query history.
                        history_text = remote.request('GET', '/v1/queries').decode()
                        assert all(r.token not in history_text for r in remotes)
                        assert remote.query('SELECT dataset FROM sentinel').to_pylist() == [{'dataset': i}]
                    evidence['independent_catalog_sentinels'] = 'passed'
                    evidence['independent_artifact_sentinels'] = 'passed'
                    evidence['credential_history_scrub'] = 'passed'
            finally:
                cleanup_errors = []
                for project in reversed(projects):
                    cmd, env = project[:2]
                    result = subprocess.run(cmd + ['down', '--volumes', '--remove-orphans'],
                        env=env, stdout=log, stderr=log)
                    if result.returncode:
                        cleanup_errors.append(cmd[3])
                if cleanup_errors:
                    raise RuntimeError('owned Compose cleanup failed: ' + ', '.join(cleanup_errors))
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(evidence, indent=2) + '\n')
        print('PASS: three dataset deployments; all six cross-dataset directions refused')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--source', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--image', help='Optional previously built image of the pinned source')
    args = parser.parse_args()
    run(args.source.resolve(), args.output.resolve(), args.image)
