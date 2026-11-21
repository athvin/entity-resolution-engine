"""The isolation probe must fail when an operation succeeds or fails wrongly."""
import unittest

from compatibility import RemoteError
from isolation import refused


class RefusalTests(unittest.TestCase):
    def test_success_is_not_isolation(self):
        with self.assertRaises(AssertionError):
            refused(lambda: None, {401, 403})

    def test_internal_error_is_not_authorization(self):
        def error():
            raise RemoteError(500, 'internal error')
        with self.assertRaises(AssertionError):
            refused(error, {401, 403})

    def test_expected_refusal(self):
        def error():
            raise RemoteError(403, 'forbidden')
        refused(error, {401, 403})
