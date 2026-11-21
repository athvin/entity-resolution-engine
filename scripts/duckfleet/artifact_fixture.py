"""Restricted identities for disposable RustFS artifact proofs only."""
import json
import secrets
import subprocess

import boto3
from botocore.config import Config
from botocore.exceptions import ClientError


def identities():
    return {role: (secrets.token_hex(12), secrets.token_hex(24))
            for role in ['publisher', 'worker']}


def client(endpoint, key, secret):
    return boto3.client('s3', endpoint_url=endpoint, region_name='us-east-1',
                        aws_access_key_id=key, aws_secret_access_key=secret,
                        config=Config(connect_timeout=2, read_timeout=5,
                                      retries={'max_attempts': 0}))


def configure(mc, scratch, endpoint, bucket, prefix, root, password, users):
    """Mirror the object/list grants and immutable deny, without AWS-only KMS.

    The endpoint comes only from docker port for a newly created fixture. The
    caller supplies random identities that it also projects into its worker.
    """
    def command(*args):
        result = subprocess.run([str(mc), '--config-dir', str(scratch), *args],
                                capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError('local mc fixture setup failed')

    command('alias', 'set', 'proof', endpoint, root, password)
    arn = 'arn:aws:s3:::' + bucket
    immutable = arn + '/' + prefix + 'artifacts/immutable/*'
    for role, (key, secret) in users.items():
        command('admin', 'user', 'add', 'proof', key, secret)
        statements = [{
            'Effect': 'Allow', 'Resource': [arn + '/' + prefix + '*'],
            'Action': ['s3:GetObject', 's3:PutObject', 's3:DeleteObject',
                       's3:AbortMultipartUpload', 's3:ListMultipartUploadParts'],
        }, {
            'Effect': 'Allow', 'Action': ['s3:ListBucket'], 'Resource': [arn],
            'Condition': {'StringLike': {'s3:prefix': [prefix + '*']}},
        }, {
            'Effect': 'Allow', 'Action': ['s3:GetBucketLocation'], 'Resource': [arn],
        }]
        if role == 'worker':
            statements.append({
                'Effect': 'Deny', 'Resource': [immutable],
                'Action': ['s3:PutObject', 's3:DeleteObject', 's3:DeleteObjectVersion',
                           's3:AbortMultipartUpload'],
            })
        path = scratch / (role + '.json')
        path.write_text(json.dumps({'Version': '2012-10-17', 'Statement': statements}))
        command('admin', 'policy', 'create', 'proof', role, str(path))
        command('admin', 'policy', 'attach', 'proof', role, '--user', key)
    retention = {
        'Effect': 'Deny', 'Principal': '*', 'Resource': [immutable],
        'Action': ['s3:DeleteObject', 's3:DeleteObjectVersion'],
    }
    conditional = {
        'Effect': 'Deny', 'Principal': '*', 'Action': ['s3:PutObject'],
        'Resource': [immutable], 'Condition': {'Null': {'s3:if-none-match': 'true'}},
    }
    admin = client(endpoint, root, password)
    try:
        admin.put_bucket_policy(Bucket=bucket, Policy=json.dumps({
            'Version': '2012-10-17', 'Statement': [retention, conditional]}))
        return 'supported'
    except ClientError as error:
        assert error.response['Error']['Code'] == 'InvalidArgument'
        assert 's3:if-none-match' in error.response['Error']['Message']
        admin.put_bucket_policy(Bucket=bucket, Policy=json.dumps({
            'Version': '2012-10-17', 'Statement': [retention]}))
        return 'unsupported by RustFS 1.0.4; requires AWS qualification'
