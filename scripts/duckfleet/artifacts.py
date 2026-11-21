#!/usr/bin/env python3
"""Run the Rust artifact transfer proof against an isolated, disposable fleet."""
import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[2]


def run(source, image, output):
    project = 'er-duckfleet-artifacts-' + uuid.uuid4().hex[:12]
    output.parent.mkdir(parents=True, exist_ok=True)
    token = secrets.token_hex(32)
    password = secrets.token_hex(32)
    dsn = f'host=postgres port=5432 user=duckfleet password={password}'
    env = dict(os.environ, DUCKFLEET_PORT='0',
               DUCKFLEET_WORKER_URLS='http://worker-small:8080')
    with tempfile.TemporaryDirectory(prefix=project) as scratch:
        scratch = Path(scratch)
        override = scratch / 'compose.json'
        override.write_text(json.dumps({'services': {
            'postgres': {'environment': {'POSTGRES_PASSWORD': password}},
            'object-store': {'ports': ['127.0.0.1::9000']},
            'worker-small': {'image': image, 'pull_policy': 'never', 'environment': {
                'DUCKFLEET_TOKEN': token, 'CATALOG_DSN': dsn + ' dbname=lake',
                'PLATFORM_DSN': dsn + ' dbname=platform'}},
            'server': {'image': image, 'pull_policy': 'never', 'environment': {
                'DUCKFLEET_TOKEN': token, 'PLATFORM_DSN': dsn + ' dbname=platform'}}}}))
        override.chmod(0o600)
        command = ['docker', 'compose', '--project-name', project, '--file',
                   str(source / 'compose.yaml'), '--file', str(override)]
        with output.with_suffix('.log').open('w') as log:
            try:
                subprocess.run(command + ['up', '--detach', '--wait', 'server'],
                               env=env, stdout=log, stderr=log, check=True)
                address = subprocess.check_output(command + ['port', 'server', '8080'],
                                                  env=env, text=True).strip()
                store_address = subprocess.check_output(command + ['port', 'object-store', '9000'], env=env, text=True).strip()
                proof_env = dict(env, ER_PROOF_ADDRESS=address, ER_PROOF_TOKEN=token,
                                 ER_PROOF_S3_ADDRESS=store_address, ER_PROOF_S3_KEY='DUCKFLEETLOCAL',
                                 ER_PROOF_S3_SECRET='local-object-store-password',
                                 ER_PROOF_DIRECTORY=str(scratch))
                result = subprocess.run(['cargo', 'run', '-p', 'er-server',
                    '--features', 'disposable-proof', '--example', 'artifact-proof'],
                    cwd=ROOT / 'rust', env=proof_env, stdout=subprocess.PIPE,
                    stderr=log, text=True, check=True)
                evidence = json.loads(result.stdout.strip().splitlines()[-1])
                worker = json.loads(subprocess.check_output(command + ['exec', '-T',
                    'worker-small', 'sh', '-c',
                    'curl -fsS http://localhost:8080/v1/worker -H "Authorization: Bearer $DUCKFLEET_TOKEN"'],
                    env=env, text=True))
                assert worker['open_sessions'] == 0, 'session cleanup leaked a worker session'
                assert worker['active_queries'] == 0, 'query remained active after cleanup'
                evidence['worker_sessions_after_close'] = worker['open_sessions']
                evidence['upstream_revision'] = subprocess.check_output(
                    ['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
                evidence['image_id'] = subprocess.check_output(
                    ['docker', 'image', 'inspect', image, '--format', '{{.Id}}'], text=True).strip()
                evidence['scope'] = 'Rust artifact transfer; disposable Compose with root S3 fixture; separate worker-policy probe; not AWS qualification'
                output.write_text(json.dumps(evidence, indent=2) + '\n')
                print(json.dumps(evidence, indent=2))
            finally:
                subprocess.run(command + ['down', '--volumes', '--remove-orphans'],
                               env=env, stdout=log, stderr=log, check=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', required=True, type=Path)
    parser.add_argument('--image', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    run(args.source.resolve(), args.image, args.output.resolve())
