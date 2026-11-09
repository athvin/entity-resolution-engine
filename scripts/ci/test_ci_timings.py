import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('timings', Path(__file__).with_name('ci-timings.py'))
timings = importlib.util.module_from_spec(spec)
spec.loader.exec_module(timings)


class Timings(unittest.TestCase):
    def test_compilation_and_execution_separate(self):
        log = '''Finished `test` profile [unoptimized + debuginfo] target(s) in 1m 23s
Summary [ 233.964s] 19 tests run: 19 passed
Finished `test` profile [unoptimized + debuginfo] target(s) in 0.20s
test result: ok. 5 passed; finished in 0.13s'''
        self.assertEqual(timings.durations(log), (83.2, 234.094))

    def test_invalid_skipped_timestamps_are_not_negative_duration(self):
        self.assertEqual(timings.seconds('2026-11-09T06:08:37Z', '2026-11-09T05:47:48Z'), 0)
        self.assertEqual(timings.seconds(None, None), 0)
