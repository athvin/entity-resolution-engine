"""Exercise CI partitioning through pytest's actual collection and marker hooks."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest


def collect(root: Path, *options: str) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ)
    environment["PYTHONPATH"] = str(Path(__file__).resolve().parents[1])
    return subprocess.run(
        [
            sys.executable,
            "-m",
            "pytest",
            "-p",
            "helpers.sharding",
            "--collect-only",
            "-q",
            *options,
        ],
        cwd=root,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )


def test_shards_cover_every_eligible_test_once(tmp_path: Path) -> None:
    (tmp_path / "pytest.ini").write_text("[pytest]\nmarkers = slow: deliberate slow case\n")
    (tmp_path / "test_cases.py").write_text(
        "import pytest\n"
        "@pytest.mark.parametrize('case', range(17))\n"
        "def test_case(case): pass\n"
        "@pytest.mark.slow\n"
        "def test_slow(): pass\n"
    )
    baseline = collect(tmp_path, "-m", "not slow")
    assert baseline.returncode == 0, baseline.stdout + baseline.stderr
    expected = {line for line in baseline.stdout.splitlines() if line.startswith("test_cases.py::")}
    assert len(expected) == 17
    seen: set[str] = set()
    sizes = []
    for index in range(3):
        result = collect(
            tmp_path,
            "-m",
            "not slow",
            "--integration-shard-index",
            str(index),
            "--integration-shard-count",
            "3",
        )
        assert result.returncode == 0, result.stdout + result.stderr
        selected = {
            line for line in result.stdout.splitlines() if line.startswith("test_cases.py::")
        }
        assert not seen & selected, "two CI jobs would run the same test"
        seen.update(selected)
        sizes.append(len(selected))
    assert seen == expected, "CI would omit an eligible test"
    assert max(sizes) - min(sizes) <= 1


@pytest.mark.parametrize(
    "options",
    [
        ["--integration-shard-index", "0"],
        ["--integration-shard-count", "3"],
        ["--integration-shard-index", "-1", "--integration-shard-count", "3"],
        ["--integration-shard-index", "3", "--integration-shard-count", "3"],
        ["--integration-shard-index", "0", "--integration-shard-count", "0"],
    ],
)
def test_invalid_shard_configuration_fails(tmp_path: Path, options: list[str]) -> None:
    (tmp_path / "test_cases.py").write_text("def test_case(): pass\n")
    result = collect(tmp_path, *options)
    assert result.returncode == 4
    assert "0 <= index < count" in result.stderr
