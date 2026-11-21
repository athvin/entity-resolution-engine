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

import boto3
from botocore.config import Config
from botocore.exceptions import BotoCoreError, ClientError


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

        def client(key, secret):
            return boto3.client(
                's3', endpoint_url=endpoint, region_name='us-east-1',
                aws_access_key_id=key, aws_secret_access_key=secret,
                config=Config(connect_timeout=2, read_timeout=5,
                              retries={'max_attempts': 0}))

        admin = client(root, password)
        for _ in range(50):
            try:
                admin.list_buckets()
                break
            except (ClientError, BotoCoreError):
                time.sleep(.2)
        else:
            raise RuntimeError('local fixture unhealthy')
        bucket = 'artifact-proof'
        arn = 'arn:aws:s3:::' + bucket
        admin.create_bucket(Bucket=bucket)
        admin.put_object(Bucket=bucket, Key='dataset-b/immutable/test',
                         Body=b'foreign synthetic input')
        with tempfile.TemporaryDirectory() as scratch:
            def command(*args):
                result = subprocess.run([mc, '--config-dir', scratch, *args],
                                        capture_output=True, text=True)
                if result.returncode:
                    raise RuntimeError('local mc fixture setup failed')

            command('alias', 'set', 'proof', endpoint, root, password)
            users = {}
            for who in ['publisher', 'worker']:
                key, secret = secrets.token_hex(12), secrets.token_hex(24)
                command('admin', 'user', 'add', 'proof', key, secret)
                statements = [{
                    'Effect': 'Allow',
                    'Action': ['s3:GetObject', 's3:PutObject', 's3:DeleteObject'],
                    'Resource': [arn + '/dataset-a/*'],
                }]
                if who == 'worker':
                    statements.append({
                        'Effect': 'Deny', 'Resource': [arn + '/dataset-a/immutable/*'],
                        'Action': ['s3:PutObject', 's3:DeleteObject', 's3:DeleteObjectVersion'],
                    })
                path = scratch + '/' + who + '.json'
                with open(path, 'w') as file:
                    json.dump({'Version': '2012-10-17', 'Statement': statements}, file)
                command('admin', 'policy', 'create', 'proof', who, path)
                command('admin', 'policy', 'attach', 'proof', who, '--user', key)
                users[who] = client(key, secret)
            retention = {
                'Effect': 'Deny', 'Principal': '*',
                'Action': ['s3:DeleteObject', 's3:DeleteObjectVersion'],
                'Resource': [arn + '/dataset-a/immutable/*'],
            }
            conditional = {
                'Effect': 'Deny', 'Principal': '*', 'Action': ['s3:PutObject'],
                'Resource': [arn + '/dataset-a/immutable/*'],
                'Condition': {'Null': {'s3:if-none-match': 'true'}},
            }
            results = {}
            try:
                admin.put_bucket_policy(Bucket=bucket, Policy=json.dumps({
                    'Version': '2012-10-17', 'Statement': [retention, conditional]}))
                results['conditional_policy_enforcement'] = 'supported'
            except ClientError as error:
                assert error.response['Error']['Code'] == 'InvalidArgument'
                assert 's3:if-none-match' in error.response['Error']['Message']
                results['conditional_policy_enforcement'] = (
                    'unsupported by RustFS 1.0.4; requires AWS qualification')
                admin.put_bucket_policy(Bucket=bucket, Policy=json.dumps({
                    'Version': '2012-10-17', 'Statement': [retention]}))
            publisher, worker = users['publisher'], users['worker']
            sealed = {'Bucket': bucket, 'Key': 'dataset-a/immutable/test'}

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
                Bucket=bucket, Key='dataset-a/immutable/new', Body=b'changed',
                IfNoneMatch='*'), 'AccessDenied')
            attempt('worker_staging', lambda: worker.put_object(
                Bucket=bucket, Key='dataset-a/staging/test', Body=b'output'), 'allowed')
            attempt('foreign_read', lambda: worker.get_object(
                Bucket=bucket, Key='dataset-b/immutable/test'), 'AccessDenied')
            return results
    finally:
        subprocess.run(['docker', 'rm', '-f', name], check=True,
                       stdout=subprocess.DEVNULL)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mc', required=True)
    args = parser.parse_args()
    print(json.dumps(run(args.mc), indent=2))
