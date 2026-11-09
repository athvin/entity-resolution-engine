#!/usr/bin/env python3
"""Judge an archive experiment; emit measurements, never change CI automatically."""
import concurrent.futures
from datetime import datetime
import json
import re
import statistics
import subprocess
import sys


def seconds(job):
    return (datetime.fromisoformat(job['completed_at'].replace('Z', '+00:00')) -
            datetime.fromisoformat(job['started_at'].replace('Z', '+00:00'))).total_seconds()


def passed(log):
    return sorted(re.findall(r'\bPASS\s+\[[^\]]+\]\s+\([^)]*\)\s+(\S+\s+\S+)', log))


def verdict(jobs, logs):
    by_name = {j['name']: j for j in jobs}
    pairs = []
    for pair in range(1, 6):
        producer = by_name[f'archive producer ({pair})']
        baseline = [by_name[f'baseline ({pair}, {shard}/4)'] for shard in range(1, 5)]
        consumers = [by_name[f'archive consumer ({pair}, {shard}/4)'] for shard in range(1, 5)]
        for job in [producer, *baseline, *consumers]:
            if job['conclusion'] != 'success':
                raise ValueError(f"incomplete or failed: {job['name']}")
        for old, new in zip(baseline, consumers):
            names = passed(logs[old['id']])
            if not names or names != passed(logs[new['id']]):
                raise ValueError(f"test identities differ or absent: {old['name']}")
        before = max(map(seconds, baseline))
        after = seconds(producer) + max(map(seconds, consumers))
        pairs.append({'pair': pair, 'baseline_seconds': before, 'archive_seconds': after})
    before = statistics.median(p['baseline_seconds'] for p in pairs)
    after = statistics.median(p['archive_seconds'] for p in pairs)
    improvement = 1 - after / before
    return {'pairs': pairs, 'median_baseline_seconds': before, 'median_archive_seconds': after,
            'improvement_percent': round(100*improvement, 2), 'adopt': improvement >= .15,
            'note': 'Per-pair critical path includes producer setup/upload plus slowest consumer setup/download/execution. Excludes unrelated runner-queue contention between the five concurrent pairs. All passed test identities match.'}


def main():
    prefix = 'repos/athvin/entity-resolution-engine/actions'
    run_id = int(sys.argv[1])
    def api(path):
        return json.loads(subprocess.check_output(['gh', 'api', prefix + path], text=True))
    run = api(f'/runs/{run_id}')
    if run['run_attempt'] != 1:
        raise ValueError('reruns cannot demonstrate a failure-free experiment')
    jobs = api(f'/runs/{run_id}/jobs?per_page=100')['jobs']
    if len(jobs) != 45 or any(j['conclusion'] != 'success' for j in jobs):
        raise ValueError('all 45 experiment jobs must succeed before comparison')
    def log(job):
        return job['id'], subprocess.check_output(['gh', 'api', f"{prefix}/jobs/{job['id']}/logs"], text=True)
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        logs = dict(pool.map(log, [j for j in jobs if 'producer' not in j['name']]))
    print(json.dumps(verdict(jobs, logs), indent=2))


if __name__ == '__main__':
    main()
