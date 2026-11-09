#!/usr/bin/env python3
"""Stage ALB registration before selecting the new colour and retiring the old.

Schema hooks and preview smoke already passed in the inactive-colour step.
Selector-only upgrades must not repeat tenant migrations during public cutover.
EKS remains the only owner of target registration.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def command(args):
    return subprocess.check_output(args, text=True, timeout=660).strip()


class Cutover:
    def __init__(self, env):
        self.env = env
        self.namespace = env['CI_NAMESPACE']
        self.live, self.target = env['LIVE'], env['TARGET']
        if {self.live, self.target} != {'blue', 'green'}:
            raise ValueError('cutover requires distinct blue/green colours')
        self.group = env.get('TARGET_GROUP_ARN', '')
        self.url = env.get('PUBLIC_HEALTH_URL', '')
        if env.get('DEPLOY_ENV') == 'prod' and not self.group:
            raise ValueError('production cutover requires the ALB target group')
        if self.group and not self.url.startswith('https://'):
            raise ValueError('ALB cutover requires an HTTPS PUBLIC_HEALTH_URL')

    def kube(self, *args):
        return json.loads(command(['kubectl', '-n', self.namespace, *args, '-o', 'json']))

    def pod_ips(self, color):
        pods = self.kube('get', 'pods', '-l',
                         'app.kubernetes.io/name=web-api,'
                         'app.kubernetes.io/instance=er-platform,'
                         f'er.dupezero.com/color={color}')['items']
        if not pods or any(p.get('metadata', {}).get('deletionTimestamp') or
                           p['status'].get('phase') != 'Running' or
                           not any(c['type'] == 'Ready' and c['status'] == 'True'
                                   for c in p['status'].get('conditions', [])) or
                           not p['status'].get('podIP') for p in pods):
            raise RuntimeError(f'{color} must have ready, non-terminating pods')
        return {p['status']['podIP'] for p in pods}

    def targets(self):
        response = json.loads(command(['aws', 'elbv2', 'describe-target-health',
                                       '--target-group-arn', self.group, '--output', 'json']))
        return {t['Target']['Id']: t['TargetHealth']['State']
                for t in response['TargetHealthDescriptions'] if t['Target']['Port'] == 3000}

    def upgrade(self, active, overlap=False):
        e = self.env
        values = {
            'developer': self.namespace, 'images.api.digest': e['NEW_API'],
            'images.web.digest': e['NEW_WEB'], 'efs.fileSystemId': e['FS'],
            'efs.accessPointId': e['AP'], 'color.active': active,
            f'color.{self.target}.replicas': '1', f'color.{self.live}.replicas': '1',
            f'color.{self.target}.api.digest': e['NEW_API'],
            f'color.{self.target}.web.digest': e['NEW_WEB'],
            f'color.{self.live}.api.digest': e['LIVE_API'],
            f'color.{self.live}.web.digest': e['LIVE_WEB'],
            'edge.overlap': str(overlap).lower(),
        }
        args = ['helm', 'upgrade', 'er-platform', 'infra/k8s/charts/er-platform',
                '--namespace', self.namespace, '--atomic', '--wait', '--timeout', '10m',
                '--no-hooks']
        if e.get('PLATFORM_VALUES'):
            args += ['-f', e['PLATFORM_VALUES']]
        if self.group:
            values['edge.targetGroupArn'] = self.group
        for key, value in values.items():
            args += ['--set', f'{key}={value}']
        print(command(args), flush=True)

    def wait_healthy(self, ips):
        deadline = time.monotonic() + 300
        while True:
            states = self.targets()
            if all(states.get(ip) == 'healthy' for ip in ips):
                print(f'ALB reports {len(ips)} expected pod targets healthy', flush=True)
                return
            if time.monotonic() >= deadline:
                raise RuntimeError('expected pod IPs did not become healthy ALB targets')
            time.sleep(5)

    def public_ready(self):
        # Reject errors and redirects; a generic edge 200 is not readiness.
        response = command(['curl', '--fail', '--silent', '--show-error',
                            '--max-time', '10', '--write-out', '\n%{http_code}', self.url])
        payload, _, status = response.rpartition('\n')
        if status != '200':
            raise RuntimeError('public readiness did not return HTTP 200')
        body = json.loads(payload)
        if body.get('ready') is not True or body.get('upstream') != 'ok':
            raise RuntimeError('public readiness response is not ready')

    def verify_public(self, ips):
        deadline = time.monotonic() + 180
        while True:
            states = self.targets()
            if any(states.get(ip) != 'healthy' for ip in ips):
                raise RuntimeError('replacement ALB target lost health after cutover')
            self.public_ready()
            if time.monotonic() >= deadline:
                print('public readiness stayed healthy for three minutes', flush=True)
                return
            time.sleep(5)

    def wait_drained(self, ips):
        deadline = time.monotonic() + 300
        while True:
            states = self.targets()
            if all(states.get(ip) in (None, 'unused') for ip in ips):
                return
            if time.monotonic() >= deadline:
                raise RuntimeError('old targets are still registered or draining; retain old pods')
            time.sleep(5)

    def selected_color(self):
        return self.kube('get', 'svc', 'er-web')['spec']['selector'].get('er.dupezero.com/color')

    def execute(self):
        old_ips = new_ips = set()
        if self.group:
            # Check permission and old health BEFORE any selector mutation.
            old_ips, new_ips = self.pod_ips(self.live), self.pod_ips(self.target)
            if old_ips & new_ips:
                raise RuntimeError('old and replacement pod IPs must be distinct')
            self.wait_healthy(old_ips)
            self.public_ready()
        try:
            if self.group:
                self.upgrade(self.live, overlap=True)
                self.wait_healthy(old_ips | new_ips)
            self.upgrade(self.target)
            if self.selected_color() != self.target:
                raise RuntimeError('Service did not select the replacement colour')
            if self.group:
                self.verify_public(new_ips)
                self.wait_drained(old_ips)
        except Exception:
            print('::error::cutover verification failed; restoring the old colour', flush=True)
            try:
                if self.group:
                    self.upgrade(self.live, overlap=True)
                    self.wait_healthy(old_ips)
                self.upgrade(self.live)
                if self.selected_color() != self.live:
                    raise RuntimeError('rollback Service selector did not converge')
                if self.group:
                    self.public_ready()
                print('old colour restored; neither deployment was retired', flush=True)
            except Exception as rollback_error:
                print(f'::error::rollback needs operator attention: {type(rollback_error).__name__}',
                      flush=True)
            raise
        print(f'traffic is verified on {self.target}; old targets drained', flush=True)
        with Path(self.env['GITHUB_STEP_SUMMARY']).open('a') as summary:
            summary.write(f'### Cut over to `{self.target}`\n\n'
                          'Selector verified; ALB/public checks completed where configured.\n'
                          f'The `{self.live}` deployment can now be retired.\n')


if __name__ == '__main__':
    try:
        Cutover(os.environ).execute()
    except Exception as error:
        print(f'::error::cutover failed: {error}', file=sys.stderr)
        sys.exit(1)
