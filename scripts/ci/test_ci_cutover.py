"""Cutover failure injection, without AWS credentials or a Kubernetes cluster."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('cutover', Path(__file__).with_name('deploy-cutover.py'))
cutover = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cutover)


class CutoverTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.env = dict(CI_NAMESPACE='app', LIVE='blue', TARGET='green', DEPLOY_ENV='prod',
                        TARGET_GROUP_ARN='arn:test', PUBLIC_HEALTH_URL='https://test.invalid/api/healthz',
                        NEW_API='sha256:newapi', NEW_WEB='sha256:newweb', LIVE_API='sha256:oldapi',
                        LIVE_WEB='sha256:oldweb', FS='fs-test', AP='fsap-test',
                        GITHUB_STEP_SUMMARY=str(Path(self.tmp.name) / 'summary'))
        self.gate = cutover.Cutover(self.env)

    def test_unrelated_healthy_target_does_not_admit_replacement(self):
        with patch.object(self.gate, 'targets', return_value={'old': 'healthy'}), \
             patch.object(cutover.time, 'monotonic', side_effect=[0, 301]):
            with self.assertRaisesRegex(RuntimeError, 'did not become healthy'):
                self.gate.wait_healthy({'old', 'replacement'})

    def test_waits_for_initial_target_registration(self):
        with patch.object(self.gate, 'targets', side_effect=[
                {'old': 'healthy', 'new': 'initial'}, {'old': 'healthy', 'new': 'healthy'}]), \
             patch.object(cutover.time, 'sleep') as sleep:
            self.gate.wait_healthy({'old', 'new'})
            sleep.assert_called_once_with(5)

    def test_draining_target_prevents_retirement(self):
        with patch.object(self.gate, 'targets', return_value={'old': 'draining'}), \
             patch.object(cutover.time, 'monotonic', side_effect=[0, 301]):
            with self.assertRaisesRegex(RuntimeError, 'retain old pods'):
                self.gate.wait_drained({'old'})

    def test_http_failure_and_generic_200_are_not_readiness(self):
        for response in ['{}\n200', '{"ready":false,"upstream":"ok"}\n200',
                         '{"ready":true,"upstream":"unavailable"}\n200',
                         '{"ready":true,"upstream":"ok"}\n302']:
            with self.subTest(response=response), patch.object(cutover, 'command', return_value=response):
                with self.assertRaises(RuntimeError):
                    self.gate.public_ready()
        with patch.object(cutover, 'command', side_effect=subprocess.CalledProcessError(22, 'curl')):
            with self.assertRaises(subprocess.CalledProcessError):
                self.gate.public_ready()

    def test_unready_and_terminating_pods_rejected(self):
        for pod in [dict(metadata={}, status=dict(phase='Running', podIP='new', conditions=[])),
                    dict(metadata=dict(deletionTimestamp='now'), status=dict(
                        phase='Running', podIP='new', conditions=[dict(type='Ready', status='True')]))]:
            with self.subTest(pod=pod), patch.object(self.gate, 'kube', return_value={'items': [pod]}):
                with self.assertRaises(RuntimeError):
                    self.gate.pod_ips('green')

    def test_failed_public_check_restores_old_colour_and_fails_release(self):
        operations = []
        with patch.object(self.gate, 'pod_ips', side_effect=[{'old'}, {'new'}]), \
             patch.object(self.gate, 'wait_healthy', side_effect=lambda ips: operations.append(('healthy', ips))), \
             patch.object(self.gate, 'public_ready'), \
             patch.object(self.gate, 'upgrade', side_effect=lambda c, overlap=False: operations.append((c, overlap))), \
             patch.object(self.gate, 'selected_color', side_effect=['green', 'blue']), \
             patch.object(self.gate, 'verify_public', side_effect=RuntimeError('public 503')), \
             patch.object(self.gate, 'wait_drained') as drain:
            with self.assertRaisesRegex(RuntimeError, 'public 503'):
                self.gate.execute()
        self.assertEqual(operations, [('healthy', {'old'}), ('blue', True),
                                     ('healthy', {'old', 'new'}), ('green', False),
                                     ('blue', True), ('healthy', {'old'}), ('blue', False)])
        drain.assert_not_called()
        self.assertFalse(Path(self.env['GITHUB_STEP_SUMMARY']).exists())

    def test_permission_failure_does_not_change_selectors(self):
        with patch.object(self.gate, 'pod_ips', side_effect=[{'old'}, {'new'}]), \
             patch.object(self.gate, 'targets', side_effect=subprocess.CalledProcessError(1, 'aws')), \
             patch.object(self.gate, 'upgrade') as upgrade:
            with self.assertRaises(subprocess.CalledProcessError):
                self.gate.execute()
            upgrade.assert_not_called()

    def test_public_window_rechecks_target_health_and_http(self):
        with patch.object(self.gate, 'targets', return_value={'new': 'healthy'}) as health, \
             patch.object(self.gate, 'public_ready') as public, \
             patch.object(cutover.time, 'monotonic', side_effect=[0, 0, 90, 180]), \
             patch.object(cutover.time, 'sleep'):
            self.gate.verify_public({'new'})
        self.assertEqual(health.call_count, 3)
        self.assertEqual(public.call_count, 3)

    def test_success_requires_public_verification_then_drain(self):
        events = []
        with patch.object(self.gate, 'pod_ips', side_effect=[{'old'}, {'new'}]), \
             patch.object(self.gate, 'wait_healthy'), patch.object(self.gate, 'public_ready'), \
             patch.object(self.gate, 'upgrade'), patch.object(self.gate, 'selected_color', return_value='green'), \
             patch.object(self.gate, 'verify_public', side_effect=lambda ips: events.append(('public', ips))), \
             patch.object(self.gate, 'wait_drained', side_effect=lambda ips: events.append(('drained', ips))):
            self.gate.execute()
        self.assertEqual(events, [('public', {'new'}), ('drained', {'old'})])
        self.assertIn('can now be retired', Path(self.env['GITHUB_STEP_SUMMARY']).read_text())

    def test_selector_upgrade_keeps_both_digests_and_skips_repeat_hooks(self):
        with patch.object(cutover, 'command', return_value='upgraded') as run:
            self.gate.upgrade('blue', overlap=True)
        args = run.call_args.args[0]
        for required in ['--no-hooks', 'edge.overlap=true', 'color.active=blue',
                         'color.blue.api.digest=sha256:oldapi', 'color.green.api.digest=sha256:newapi',
                         'color.blue.replicas=1', 'color.green.replicas=1']:
            self.assertIn(required, args)

    def test_prod_cannot_silently_skip_edge_verification(self):
        for key in ['TARGET_GROUP_ARN', 'PUBLIC_HEALTH_URL']:
            with self.subTest(key=key), self.assertRaises(ValueError):
                cutover.Cutover({**self.env, key: ''})


if __name__ == '__main__':
    unittest.main()
