#!/usr/bin/env python3
"""Read attempt-specific Actions timings, keeping retries and setup visible.

Usage: python3 scripts/ci/ci-timings.py --workflow rust.yml --limit 20 [--logs]
Logs are reduced to numeric compiler/test summaries; payloads are never emitted.
"""
import argparse
from datetime import datetime
import json
import math
import re
import statistics
import subprocess


def api(path):
    return json.loads(subprocess.check_output(['gh', 'api', path], text=True))


def seconds(start, finish):
    if not start or not finish or start.startswith('0001') or finish.startswith('0001'):
        return 0
    return max(0, (datetime.fromisoformat(finish.replace('Z', '+00:00')) -
                   datetime.fromisoformat(start.replace('Z', '+00:00'))).total_seconds())


def durations(log):
    compile_time = 0.0
    for elapsed in re.findall(r'Finished `[^`]+` .*? in ([0-9hms. ]+)', log):
        compile_time += sum(float(value) * {'h':3600, 'm':60, 's':1}[unit]
                            for value, unit in re.findall(r'(\d+(?:\.\d+)?)([hms])', elapsed))
    test_time = sum(float(n) for n in re.findall(r'test result:.*?finished in ([0-9.]+)s', log))
    test_time += sum(float(n) for n in re.findall(r'Summary \[\s*([0-9.]+)s\]', log))
    return compile_time, test_time


def summarize(jobs):
    result = []
    for job in jobs:
        if job['conclusion'] == 'skipped':
            continue
        steps = [{'name': s['name'], 'seconds': seconds(s.get('started_at'), s.get('completed_at'))}
                 for s in job.get('steps', [])]
        result.append({'id': job['id'], 'name': job['name'], 'conclusion': job['conclusion'],
                       'queue_seconds': seconds(job.get('created_at'), job.get('started_at')),
                       'seconds': seconds(job.get('started_at'), job.get('completed_at')), 'steps': steps})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', default='athvin/entity-resolution-engine')
    parser.add_argument('--workflow', default='rust.yml')
    parser.add_argument('--limit', type=int, default=20)
    parser.add_argument('--logs', action='store_true')
    args = parser.parse_args()
    prefix = f'repos/{args.repo}/actions'
    runs = api(f'{prefix}/workflows/{args.workflow}/runs?per_page={args.limit}')['workflow_runs']
    records = []
    for run in runs:
        if run['status'] != 'completed':
            continue
        attempts = []
        for attempt in range(1, run['run_attempt'] + 1):
            meta = api(f"{prefix}/runs/{run['id']}/attempts/{attempt}")
            jobs = api(f"{prefix}/runs/{run['id']}/attempts/{attempt}/jobs?per_page=100")['jobs']
            rows = summarize(jobs)
            if args.logs:
                for row in rows:
                    log = subprocess.run(['gh', 'api', f"{prefix}/jobs/{row['id']}/logs"], text=True, capture_output=True)
                    if log.returncode == 0:
                        row['compile_seconds_sum'], row['test_execution_seconds_sum'] = durations(log.stdout)
            attempts.append({'attempt': attempt, 'started_at': meta['run_started_at'],
                             'jobs': rows, 'conclusion': meta['conclusion']})
        records.append({'id': run['id'], 'event': run['event'], 'sha': run['head_sha'],
                        'conclusion': run['conclusion'], 'attempts': attempts,
                        'time_to_final_result_seconds': seconds(run['created_at'], run['updated_at'])})
    elapsed = sorted(r['time_to_final_result_seconds'] for r in records if r['conclusion'] == 'success')
    print(json.dumps({'workflow': args.workflow, 'runs': records, 'successful_runs': len(elapsed),
                      'median_seconds': statistics.median(elapsed) if elapsed else None,
                      'p95_seconds': elapsed[math.ceil(.95*len(elapsed))-1] if elapsed else None,
                      'note': 'Includes retries and human rerun delay. Use attempt/job data to distinguish these from execution; skipped-only runs must not be compared with full gates.'}, indent=2))


if __name__ == '__main__':
    main()
