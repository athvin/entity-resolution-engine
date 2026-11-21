#!/usr/bin/env python3
"""Non-root artifact policy proof in a disposable local RustFS container.

Requires boto3, Docker and a separately verified MinIO mc binary. This is not
live AWS qualification. No cloud endpoint or user credential is consulted.
"""
import argparse
import json
import secrets
import subprocess
import tempfile
import time
import uuid

from pathlib import Path
from botocore.exceptions import BotoCoreError, ClientError
from artifact_fixture import client, configure, identities


def run(mc):
    name = 'er-artifact-policy-' + uuid.uuid4().hex[:10]
    root, password = secrets.token_hex(12), secrets.token_hex(24)
    subprocess.run([
        'docker', 'run', '--detach', '--name', name, '-p', '127.0.0.1::9000',
        '-e', 'RUSTFS_ACCESS_KEY=' + root, '-e', 'RUSTFS_SECRET_KEY=' + password,
        '-e', 'RUSTFS_CONSOLE_ENABLE=false', 'rustfs/rustfs:1.0.4',
    ], check=True, stdout=subprocess.DEVNULL)
    try:
        endpoint = 'http://' + subprocess.check_output(
            ['docker', 'port', name, '9000'], text=True).strip()

        admin = client(endpoint, root, password)
        for _ in range(50):
            try:
                admin.list_buckets()
                break
            except (ClientError, BotoCoreError):
                time.sleep(.2)
        else:
            raise RuntimeError('local fixture unhealthy')
        bucket = 'artifact-proof'
        admin.create_bucket(Bucket=bucket)
        admin.put_object(Bucket=bucket, Key='dataset-b/artifacts/immutable/test',
                         Body=b'foreign synthetic input')
        with tempfile.TemporaryDirectory() as scratch:
            users = identities()
            support = configure(mc, Path(scratch), endpoint, bucket, 'dataset-a/',
                                root, password, users)
            results = {'conditional_policy_enforcement': support}
            users = {role: client(endpoint, *credentials) for role, credentials in users.items()}
            publisher, worker = users['publisher'], users['worker']
            sealed = {'Bucket': bucket, 'Key': 'dataset-a/artifacts/immutable/test'}

            def attempt(label, action, expected):
                try:
                    action()
                    results[label] = 'allowed'
                except ClientError as error:
                    results[label] = error.response['Error']['Code']
                assert results[label] == expected, (label, results[label])

            attempt('publish', lambda: publisher.put_object(
                **sealed, Body=b'accepted', IfNoneMatch='*'), 'allowed')
            attempt('conditional_rewrite', lambda: publisher.put_object(
                **sealed, Body=b'changed', IfNoneMatch='*'), 'PreconditionFailed')
            attempt('publisher_delete', lambda: publisher.delete_object(**sealed), 'AccessDenied')
            attempt('worker_read', lambda: worker.get_object(**sealed)['Body'].read(), 'allowed')
            attempt('worker_delete', lambda: worker.delete_object(**sealed), 'AccessDenied')
            attempt('worker_seal', lambda: worker.put_object(
                Bucket=bucket, Key='dataset-a/artifacts/immutable/new', Body=b'changed',
                IfNoneMatch='*'), 'AccessDenied')
            attempt('worker_staging', lambda: worker.put_object(
                Bucket=bucket, Key='dataset-a/artifacts/staging/test', Body=b'output'), 'allowed')
            attempt('foreign_read', lambda: worker.get_object(
                Bucket=bucket, Key='dataset-b/artifacts/immutable/test'), 'AccessDenied')
            return results
    finally:
        subprocess.run(['docker', 'rm', '-f', name], check=True,
                       stdout=subprocess.DEVNULL)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mc', required=True)
    args = parser.parse_args()
    print(json.dumps(run(args.mc), indent=2))
