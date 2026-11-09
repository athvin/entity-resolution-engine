"""The extension gate must reuse the feature-unified binary and fail closed."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent


class SelectedTests(unittest.TestCase):
    def run_gate(self, mode="ok", extensions=True, packages=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            # A fake Cargo plus a real executable exercise the process boundary,
            # JSON mixed with libtest output, and propagation of the scoped env.
            (root / "cargo").write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
root = Path(__file__).parent
with (root / "calls").open("a") as log:
    log.write(json.dumps([sys.argv[1:], os.environ.get("DEDUPE_E2E_EXTENSIONS")])+"\\n")
if sys.argv[1] == "metadata":
    print(json.dumps({"packages": [{"name": "dedupe-core", "id": "core-id"}]}))
else:
    # Other packages also name their integration target `main`.
    print(json.dumps({"reason": "compiler-artifact", "package_id": "other-id",
                      "target": {"name": "main", "kind": ["test"]},
                      "executable": "/must-not-run-this-other-package"}))
    artifact = {"reason": "compiler-artifact", "package_id": "core-id",
                "target": {"name": "main", "kind": ["test"]},
                "executable": str(root / "core-test")}
    mode = os.environ["MODE"]
    if mode != "missing":
        print(json.dumps(artifact))
    if mode == "ambiguous":
        artifact["executable"] += "-other"
        print(json.dumps(artifact))
    print("Running current target", file=sys.stderr, flush=True)
    print("test ordinary_suite ... ok", flush=True)
    print("Running next target", file=sys.stderr, flush=True)
    sys.exit(1 if mode == "suite-fails" else 0)
''')
            (root / "core-test").write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
root = Path(__file__).parent
with (root / "calls").open("a") as log:
    log.write(json.dumps([sys.argv[1:], os.environ.get("DEDUPE_E2E_EXTENSIONS")])+"\\n")
if "--list" in sys.argv:
    if os.environ["MODE"] != "empty-filter":
        print("extensions::loads_a_community_extension_and_exposes_its_functions: test")
else:
    assert os.environ["DEDUPE_E2E_EXTENSIONS"] == "1"
    sys.exit(1 if os.environ["MODE"] == "extension-fails" else 0)
''')
            for name in ("cargo", "core-test"):
                (root / name).chmod(0o755)
            env = {**os.environ, "PATH": str(root) + os.pathsep + os.environ["PATH"],
                   "MODE": mode, "RUN_EXTENSIONS": str(extensions).lower(),
                   "SELECTED_PACKAGES": json.dumps(packages if packages is not None else ["dedupe-core", "er-sql-check"])}
            env.pop("DEDUPE_E2E_EXTENSIONS", None)
            result = subprocess.run(["python3", str(HERE / "run-selected-tests.py")],
                                    env=env, capture_output=True, text=True)
            calls = [json.loads(line) for line in (root / "calls").read_text().splitlines()] if (root / "calls").exists() else []
            return result, calls

    def test_one_build_and_only_extension_binary_receives_the_env(self):
        result, calls = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        builds = [args for args, _ in calls if args[0] == "test"]
        self.assertEqual(len(builds), 1)
        self.assertEqual(builds[0][-4:], ["-p", "dedupe-core", "-p", "er-sql-check"])
        self.assertEqual([env for _, env in calls], [None, None, None, "1"])
        self.assertIn("test ordinary_suite ... ok", result.stdout)
        self.assertLess(result.stdout.index("Running current target"),
                        result.stdout.index("test ordinary_suite ... ok"))
        self.assertLess(result.stdout.index("test ordinary_suite ... ok"),
                        result.stdout.index("Running next target"))

    def test_unselected_extension_gate_needs_no_metadata_or_binary(self):
        result, calls = self.run_gate(extensions=False, packages=["er-match"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 1)

    def test_failed_suite_never_runs_extensions(self):
        result, calls = self.run_gate("suite-fails")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(all(env is None for _, env in calls))

    def test_missing_ambiguous_empty_or_failed_extension_gate_is_red(self):
        for mode in ("missing", "ambiguous", "empty-filter", "extension-fails"):
            with self.subTest(mode=mode):
                result, _ = self.run_gate(mode)
                self.assertNotEqual(result.returncode, 0)

    def test_invalid_selection_refused_before_cargo(self):
        for packages in ([], "dedupe-core", ["--all"], ["er-match"]):
            with self.subTest(packages=packages):
                result, calls = self.run_gate(packages=packages)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls, [])
