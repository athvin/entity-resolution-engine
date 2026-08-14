"""The spec linter is wired into the unit layer, both arms.

``scripts/lint_spec.py`` is what keeps DesignDoc.md from regressing toward the
v1.0 draft the M0 amendment replaced. A linter that passes on the amended
document proves nothing on its own -- it has to *fail* on the committed v1.0
copy, which is what the ``--expect-fail`` arms below assert (S8.1, S9.1).
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
LINTER = "scripts/lint_spec.py"
DESIGN_DOC = "DesignDoc.md"
V1_0_FIXTURE = "tests/fixtures/designdoc_v1.0.md"


def run_linter(*args: str) -> subprocess.CompletedProcess[str]:
    # sys.executable is the interpreter `python3` resolves to under `uv run`, and
    # the linter is pure stdlib, so this is the S9.1 command with a pinned python.
    return subprocess.run(
        [sys.executable, LINTER, *args],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )


def test_lint_passes_on_designdoc() -> None:
    result = run_linter(DESIGN_DOC)
    assert result.returncode == 0, result.stderr


def test_lint_fails_on_v1_0_fixture() -> None:
    result = run_linter(V1_0_FIXTURE)
    assert result.returncode == 1, result.stdout


def test_expect_fail_passes_on_v1_0_fixture() -> None:
    result = run_linter("--expect-fail", V1_0_FIXTURE)
    assert result.returncode == 0, result.stderr


def test_expect_fail_fails_on_designdoc() -> None:
    # The non-vacuity arm: if the linter ever stops finding defects in the v1.0
    # baseline it would also pass here, and this is where that is caught.
    result = run_linter("--expect-fail", DESIGN_DOC)
    assert result.returncode == 1, result.stdout
