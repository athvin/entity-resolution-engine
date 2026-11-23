#!/usr/bin/env python3
"""Run the Rust generated-read proof against an isolated, disposable fleet."""
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
    project = 'er-duckfleet-reads-' + uuid.uuid4().hex[:12]
    output.parent.mkdir(parents=True, exist_ok=True)
    token = secrets.token_hex(32)
    password = secrets.token_hex(32)
    # Upstream provisions one role per component (gateway, worker, catalog, ...)
    # from this administrator password; the role DSNs come from its compose file.
    # Tokens go through upstream's own compose variables: the gateway takes the
    # public token, and an ordinary worker refuses it, accepting only the
    # internal service token.
    env = dict(os.environ, DUCKFLEET_PORT='0', DUCKFLEET_POSTGRES_ADMIN_PASSWORD=password,
               DUCKFLEET_TOKEN=token, DUCKFLEET_INTERNAL_TOKEN=secrets.token_hex(32),
               DUCKFLEET_WORKER_URLS='http://worker-small:8080')
    with tempfile.TemporaryDirectory(prefix=project) as scratch:
        scratch = Path(scratch)
        override = scratch / 'compose.json'
        override.write_text(json.dumps({'services': {
            name: {'image': image, 'pull_policy': 'never'}
            for name in ('platform-migrate', 'worker-small', 'server')}}))
        override.chmod(0o600)
        command = ['docker', 'compose', '--project-name', project, '--file',
                   str(source / 'compose.yaml'), '--file', str(override)]
        with output.with_suffix('.log').open('w') as log:
            try:
                subprocess.run(command + ['up', '--detach', '--wait', 'server'],
                               env=env, stdout=log, stderr=log, check=True)
                address = subprocess.check_output(command + ['port', 'server', '8080'],
                                                  env=env, text=True).strip()
                proof_env = dict(env, ER_PROOF_ADDRESS=address, ER_PROOF_TOKEN=token,
                                 ER_PROOF_DIRECTORY=str(scratch))
                result = subprocess.run(['cargo', 'run', '-p', 'er-duckfleet',
                    '--features', 'disposable-proof', '--example', 'read-parity'],
                    cwd=ROOT / 'rust', env=proof_env, stdout=subprocess.PIPE,
                    stderr=log, text=True, check=True)
                evidence = json.loads(result.stdout.strip().splitlines()[-1])
                worker = json.loads(subprocess.check_output(command + ['exec', '-T',
                    'worker-small', 'sh', '-c',
                    'curl -fsS http://localhost:8080/v1/worker -H "Authorization: Bearer $DUCKFLEET_INTERNAL_TOKEN"'],
                    env=env, text=True))
                assert worker['open_sessions'] == 0, 'session cleanup leaked a worker session'
                assert worker['active_queries'] == 0, 'query remained active after cleanup'
                evidence['worker_sessions_after_close'] = worker['open_sessions']
                evidence['upstream_revision'] = subprocess.check_output(
                    ['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
                evidence['image_id'] = subprocess.check_output(
                    ['docker', 'image', 'inspect', image, '--format', '{{.Id}}'], text=True).strip()
                evidence['scope'] = 'Rust generated read; disposable Compose; not production qualification'
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
