"""Execute the benchmark workflow's preflight shell under a stub `docker` (ER-101, S9.2).

The preflight fails closed when free disk is below the scale's `min_free_gb`, and on
success exports exactly the three S10.2 envelope values to `$GITHUB_ENV` (never
`ER_DUCKDB_THREADS`, which `x-er-env` derives from `ER_CPU_LIMIT`). The shell is extracted
through `benchmarks/workflow.py` and run with a stub `docker` that answers the
`scales.py --field` queries, so the test needs no Docker and no real scale arithmetic.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from types import ModuleType

REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "benchmark.yaml"


def _import(module: str) -> ModuleType:
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


workflow = _import("workflow")

#: A `docker` stand-in: it answers the `scales.py --field <f>` probes and no-ops `compose`.
#: `min_free_gb` is read from `$STUB_MIN_FREE_GB` so a test can drive it above or below the
#: real free space.
_STUB_DOCKER = """#!/bin/sh
case "$*" in
  *"--field min_free_gb"*) echo "${STUB_MIN_FREE_GB}" ;;
  *"--field cpu_limit"*) echo 2 ;;
  *"--field mem_limit"*) echo 6g ;;
  *"--field duckdb_memory_limit"*) echo 4GB ;;
  *) exit 0 ;;
esac
"""


def _run_preflight(tmp_path: Path, *, stub_min_free_gb: str) -> tuple[int, str]:
    bindir = tmp_path / "bin"
    bindir.mkdir()
    stub = bindir / "docker"
    stub.write_text(_STUB_DOCKER, encoding="utf-8")
    stub.chmod(0o755)

    github_env = tmp_path / "github_env"
    github_env.write_text("", encoding="utf-8")

    env = dict(os.environ)
    env["PATH"] = f"{bindir}:{env['PATH']}"
    env["SCALE"] = "smoke"
    env["GITHUB_ENV"] = str(github_env)
    env["STUB_MIN_FREE_GB"] = stub_min_free_gb

    script = workflow.preflight_script(WORKFLOW)
    result = subprocess.run(
        ["bash", "-c", script], capture_output=True, text=True, env=env, check=False
    )
    return result.returncode, github_env.read_text(encoding="utf-8")


def test_preflight_fails_when_free_disk_below_min_free_gb(tmp_path: Path) -> None:
    """AC8: a min_free_gb above the available space exits non-zero and writes no env."""
    code, env_text = _run_preflight(tmp_path, stub_min_free_gb="999999999")
    assert code != 0, "preflight did not fail on insufficient disk"
    assert env_text.strip() == "", f"$GITHUB_ENV was written despite the failure: {env_text!r}"


def test_preflight_exports_scale_envelope_to_github_env(tmp_path: Path) -> None:
    """AC8: below-threshold min_free_gb exits 0 and exports exactly the three envelope
    assignments with the stub's values — and never ER_DUCKDB_THREADS."""
    code, env_text = _run_preflight(tmp_path, stub_min_free_gb="0")
    assert code == 0, f"preflight failed with sufficient disk:\n{env_text}"
    lines = [line for line in env_text.splitlines() if line.strip()]
    assert set(lines) == {
        "ER_CPU_LIMIT=2",
        "ER_MEM_LIMIT=6g",
        "ER_DUCKDB_MEMORY_LIMIT=4GB",
    }, f"unexpected env exports: {lines}"
    assert not any(line.startswith("ER_DUCKDB_THREADS=") for line in lines)
