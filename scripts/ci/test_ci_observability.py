"""A fingerprint never licenses skipping an unhealthy telemetry deployment."""
import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('obs', Path(__file__).with_name('deploy-observability.py'))
obs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(obs)


def state():
    releases = [{'name': name, 'status': 'deployed'} for name in ['fluent-bit', 'otel-collector', 'otel-operator']]
    workloads = {'items': [dict(kind='Deployment', metadata=dict(name=name, generation=2, labels={'app.kubernetes.io/instance': name}),
                              spec=dict(replicas=1), status=dict(observedGeneration=2, availableReplicas=1, updatedReplicas=1))
                           for name in ['openobserve', 'otel-collector', 'otel-operator']]}
    workloads['items'].append(dict(kind='DaemonSet', metadata=dict(name='fluent-bit-aws-for-fluent-bit', generation=1, labels={'app.kubernetes.io/instance':'fluent-bit-aws-for-fluent-bit'}),
                                   spec={}, status=dict(observedGeneration=1, desiredNumberScheduled=2, numberReady=2, updatedNumberScheduled=2)))
    return releases, workloads


class Health(unittest.TestCase):
    def test_healthy(self):
        self.assertTrue(obs.healthy(*state()))

    def test_helm_release_instance_label(self):
        releases, workloads = state()
        workloads['items'][-1]['metadata']['labels']['app.kubernetes.io/instance'] = 'fluent-bit'
        self.assertTrue(obs.healthy(releases, workloads))

    def test_unrelated_daemonset_does_not_replace_fluent_bit(self):
        releases, workloads = state()
        workloads['items'][-1]['metadata']['labels']['app.kubernetes.io/instance'] = 'unrelated'
        self.assertFalse(obs.healthy(releases, workloads))

    def test_missing_release_or_workload(self):
        releases, workloads = state()
        self.assertFalse(obs.healthy(releases[:-1], workloads))
        self.assertFalse(obs.healthy(releases, {'items': workloads['items'][1:]}))

    def test_rollout_not_observed_not_ready_or_scaled_down(self):
        for key in ['observedGeneration', 'availableReplicas', 'updatedReplicas']:
            releases, workloads = state()
            workloads['items'][0]['status'][key] = 0
            self.assertFalse(obs.healthy(releases, workloads))
        releases, workloads = state()
        workloads['items'][0]['spec']['replicas'] = 0
        self.assertFalse(obs.healthy(releases, workloads))

    def test_failed_helm_release(self):
        releases, workloads = state()
        releases[0]['status'] = 'failed'
        self.assertFalse(obs.healthy(releases, workloads))

    def test_daemonset_must_finish_update(self):
        releases, workloads = state()
        workloads['items'][-1]['status']['updatedNumberScheduled'] = 1
        self.assertFalse(obs.healthy(releases, workloads))


class Reconciliation(unittest.TestCase):
    def test_bootstrap_skip_rotation_and_failed_apply(self):
        import json
        import subprocess
        from unittest.mock import patch
        releases, workloads = state()
        marker = {}
        version = ['v1']
        reconciles = []
        fail = [False]

        def command(*args):
            if args[0] == 'aws':
                return json.dumps({version[0]: ['AWSCURRENT']})
            if 'configmap' in args:
                return json.dumps(marker)
            if args[0] == 'helm':
                return json.dumps(releases)
            return json.dumps(workloads)

        def run(args, **kwargs):
            if args[0] == 'bash':
                reconciles.append(version[0])
                if fail[0]:
                    raise subprocess.CalledProcessError(1, args)
            if 'input' in kwargs:
                marker.update(json.loads(kwargs['input']))
            return subprocess.CompletedProcess(args, 0)

        with patch.object(obs, 'command', side_effect=command), patch.object(obs.subprocess, 'run', side_effect=run):
            obs.main()
            self.assertEqual(reconciles, ['v1'])
            obs.main()
            self.assertEqual(reconciles, ['v1'])
            version[0] = 'v2'
            obs.main()
            self.assertEqual(reconciles, ['v1', 'v2'])
            previous = copy.deepcopy(marker)
            version[0] = 'v3'
            fail[0] = True
            with self.assertRaises(subprocess.CalledProcessError):
                obs.main()
            self.assertEqual(marker, previous)


class EnvironmentBoundary(unittest.TestCase):
    def test_prod_reads_only_prod_secret_and_cannot_reuse_dev_marker(self):
        import json
        from unittest.mock import patch
        releases, workloads = state()
        marker, secrets, reconciles = {}, [], []

        def command(*args):
            if args[0] == 'aws':
                secrets.append(args[args.index('--secret-id') + 1])
                return json.dumps({'same-version': ['AWSCURRENT']})
            if 'configmap' in args:
                return json.dumps(marker)
            return json.dumps(releases if args[0] == 'helm' else workloads)

        def run(args, **kwargs):
            if args[0] == 'bash':
                reconciles.append(True)
            if 'input' in kwargs:
                marker.update(json.loads(kwargs['input']))

        with patch.object(obs, 'command', side_effect=command), patch.object(obs.subprocess, 'run', side_effect=run):
            with patch.dict(obs.os.environ, {'DEPLOY_ENV': 'dev'}):
                obs.main()
            with patch.dict(obs.os.environ, {'DEPLOY_ENV': 'prod'}):
                obs.main()
                obs.main()
        self.assertEqual(secrets, ['er/shared/openobserve', 'er/prod/openobserve', 'er/prod/openobserve'])
        self.assertEqual(len(reconciles), 2)

    def test_unknown_environment_fails_before_reading_credentials(self):
        from unittest.mock import patch
        with patch.dict(obs.os.environ, {'DEPLOY_ENV': 'prodd'}), patch.object(obs, 'command') as command:
            with self.assertRaises(ValueError):
                obs.main()
            command.assert_not_called()
