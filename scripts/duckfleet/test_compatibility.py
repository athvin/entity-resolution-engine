"""Negative tests: a partial or lossy probe must never report compatibility."""
import unittest
import pyarrow as pa
from compatibility import EXPECTED_PROBES, decision, equal


class CompatibilityGate(unittest.TestCase):
    def report(self):
        return {'probes': [{'name': name, 'passed': True} for name in EXPECTED_PROBES]}

    def test_complete_success(self):
        self.assertEqual(decision(self.report()), 'go')

    def test_missing_probe_is_not_success(self):
        report = self.report()
        report['probes'].pop()
        self.assertEqual(decision(report), 'no-go')

    def test_failed_probe_is_not_success(self):
        report = self.report()
        report['probes'][0]['passed'] = False
        self.assertEqual(decision(report), 'no-go')

    def test_cleanup_failure_is_not_success(self):
        report = self.report()
        report['fatal'] = 'session close failed'
        self.assertEqual(decision(report), 'no-go')

    def test_null_and_type_changes_are_detected(self):
        expected = pa.table({'value': pa.array([None, 1], type=pa.int64())})
        for changed in (pa.table({'value': [0, 1]}),
                        pa.table({'value': pa.array([None, 1], type=pa.int32())})):
            with self.subTest(schema=changed.schema):
                with self.assertRaises(AssertionError):
                    equal(expected, changed)

    def test_small_numeric_drift_is_detected(self):
        with self.assertRaises(AssertionError):
            equal(pa.table({'score': [0.05674355410260158]}),
                  pa.table({'score': [0.05674355410260159]}))


if __name__ == '__main__':
    unittest.main()
