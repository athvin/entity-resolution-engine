"""Hermetic checks for session targeting and fleet read failures."""
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("fleet", Path(__file__).with_name("fleet.py"))
fleet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fleet)


class FleetTest(unittest.TestCase):
    def test_outside_requires_target(self):
        with patch.dict(os.environ, {}, clear=True), patch("sys.argv", ["fleet"]), \
                patch("sys.stderr", io.StringIO()), patch.object(fleet.subprocess, "run") as run:
            with self.assertRaises(SystemExit) as result:
                fleet.main()
            self.assertEqual(result.exception.code, 2)
            run.assert_not_called()

    def test_explicit_session_on_list_and_reads(self):
        calls = []
        def invoke(command, **kwargs):
            calls.append(command)
            if command[-2:] == ["agent", "list"]:
                return subprocess.CompletedProcess(command, 0,
                    '{"result":{"agents":[{"pane_id":"w1:p1","agent_status":"idle"}]}}', "")
            return subprocess.CompletedProcess(command, 0, "429 Too Many Requests", "")
        output = io.StringIO()
        with patch.dict(os.environ, {"HERDR_PANE_ID": "w1:p1"}, clear=True), \
                patch("sys.argv", ["fleet", "--session", "other"]), \
                patch.object(fleet.subprocess, "run", side_effect=invoke), \
                patch("sys.stdout", output):
            fleet.main()
        self.assertEqual(len(calls), 2)
        self.assertTrue(all(c[:3] == ["herdr", "--session", "other"] for c in calls))
        self.assertIn("RATE-LIMITED", output.getvalue())
        self.assertIn("1 of 1", output.getvalue())

    def test_failed_read_is_visible(self):
        with patch.object(fleet.subprocess, "run", return_value=
                          subprocess.CompletedProcess([], 1, "", "unavailable")):
            alert, text = fleet.screen(["herdr"], "w1:p1")
        self.assertIsNone(alert)
        self.assertIn("read failed", text)


if __name__ == "__main__":
    unittest.main()
