import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('archive', Path(__file__).with_name('archive-verdict.py'))
archive = importlib.util.module_from_spec(spec)
spec.loader.exec_module(archive)


class ArchiveVerdict(unittest.TestCase):
    def data(self, producer_seconds):
        jobs, logs = [], {}
        for pair in range(1, 6):
            for name, seconds in [(f'archive producer ({pair})', producer_seconds),
                                  *[(f'{arm} ({pair}, {shard}/4)', 100 if arm == 'baseline' else 70)
                                    for arm in ['baseline', 'archive consumer'] for shard in range(1, 5)]]:
                jobs.append(dict(id=len(jobs), name=name, conclusion='success', started_at='2026-11-09T00:00:00Z',
                                 completed_at=f'2026-11-09T00:{seconds//60:02}:{seconds%60:02}Z'))
                logs[jobs[-1]['id']] = 'PASS [ 1.0s] (1/1) er-cli::main test_name'
        return jobs, logs

    def test_producer_time_counts_against_savings(self):
        self.assertFalse(archive.verdict(*self.data(20))['adopt'])
        self.assertTrue(archive.verdict(*self.data(10))['adopt'])

    def test_failure_and_missing_test_identity_refuse_adoption(self):
        jobs, logs = self.data(10)
        jobs[0]['conclusion'] = 'failure'
        with self.assertRaises(ValueError):
            archive.verdict(jobs, logs)
        jobs[0]['conclusion'] = 'success'
        logs[5] = 'PASS [ 1.0s] (1/1) er-cli::main different_test'
        with self.assertRaises(ValueError):
            archive.verdict(jobs, logs)
