"""Repo-wide fixture lint: every committed scenario satisfies the S8.2.1 format.

Discovery walks `fixtures/static/`, so a scenario authored by a later ticket
(`base_10` in ER-041, `incremental_batch` in ER-064, the merge, split, assertion,
supersession and deletion scenarios in theirs) comes under lint by existing. No
scenario is named here, and nothing in this file or in `scripts/validate_fixtures.py`
has to be edited to bring one in -- which is the only arrangement under which the
lint is still complete a dozen tickets from now.

The two valid self-test scenarios are linted alongside them, so this test has a
subject today, while `fixtures/static/` is still empty. The negative self-test
fixtures are deliberately NOT linted here: they exist to fail, and each is asserted
to fail for its own rule in `tests/unit/fixtures/test_fixture_format.py`.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

from helpers.scenario import MANIFEST_NAME, discover_scenarios

REPO_ROOT = Path(__file__).resolve().parents[2]
LINTER = REPO_ROOT / "scripts" / "validate_fixtures.py"
STATIC_ROOT = REPO_ROOT / "fixtures" / "static"
SELFTEST_ROOT = REPO_ROOT / "tests" / "fixtures" / "scenarios"

#: The self-test scenarios that are meant to pass. Named here because they are
#: this test's own material, not fixtures the pipeline ships.
VALID_SELFTEST_SCENARIOS = ("composed", "ok_minimal")


def test_every_committed_scenario_validates(tmp_path: Path) -> None:
    directories = [
        *discover_scenarios(STATIC_ROOT),
        *(SELFTEST_ROOT / name for name in VALID_SELFTEST_SCENARIOS),
    ]
    result = subprocess.run(
        [sys.executable, str(LINTER), *(str(directory) for directory in directories)],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
        check=False,
    )
    assert result.returncode == 0, result.stderr

    # Discovery is by walking, not by a list: a directory dropped into a scenario
    # root is a scenario, a loose file is not, and tooling directories are not.
    (tmp_path / "new_scenario").mkdir()
    (tmp_path / "new_scenario" / MANIFEST_NAME).touch()
    (tmp_path / "__pycache__").mkdir()
    (tmp_path / "model_test_v1.json").touch()
    assert discover_scenarios(tmp_path) == (tmp_path / "new_scenario",)
