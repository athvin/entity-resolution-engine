#!/usr/bin/env python3
"""Run the ``actionlint`` binary shipped by the pinned ``actionlint-py`` wheel.

This wrapper exists so the workflow linter is pinned by ``uv.lock`` like every
other dependency. It **downloads nothing**: a binary fetched at CI time is an
unpinned dependency whose findings change under the very gate it is guarding
(DesignDoc.md S2.1, S9.1). If the binary is missing the answer is
``uv sync --frozen``, never a fetch.

Usage:
    uv run python scripts/actionlint.py [actionlint arguments ...]

Exit codes: whatever ``actionlint`` returned; 2 if the pinned binary is absent.
"""

from __future__ import annotations

import subprocess
import sys
from importlib import metadata
from pathlib import Path

DISTRIBUTION = "actionlint-py"
BINARY_NAME = "actionlint.exe" if sys.platform == "win32" else "actionlint"

EXIT_MISSING_BINARY = 2


def binary_path() -> Path | None:
    """Locate the binary the pinned wheel installed.

    The wheel records the executable in its own file list as a path relative to
    ``site-packages`` (``../../../bin/actionlint``), so resolving it through the
    distribution binds this wrapper to the pinned wheel rather than to whatever
    ``actionlint`` happens to be first on ``PATH``.
    """
    try:
        distribution = metadata.distribution(DISTRIBUTION)
    except metadata.PackageNotFoundError:
        return None

    for entry in distribution.files or ():
        if Path(entry).name == BINARY_NAME:
            candidate = Path(distribution.locate_file(entry)).resolve()
            if candidate.exists():
                return candidate

    # Some installers rewrite the script path without updating RECORD. The
    # interpreter's own script directory is still inside the synced environment,
    # so this stays a lookup within the pinned install, not a PATH search.
    scripts_dir = Path(sys.prefix) / ("Scripts" if sys.platform == "win32" else "bin")
    fallback = scripts_dir / BINARY_NAME
    return fallback if fallback.exists() else None


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv

    binary = binary_path()
    if binary is None:
        print(
            f"actionlint: the {DISTRIBUTION} wheel is not installed, so the pinned binary is "
            "unavailable. Run 'uv sync --frozen'. This wrapper never downloads one.",
            file=sys.stderr,
        )
        return EXIT_MISSING_BINARY

    completed = subprocess.run([str(binary), *args])
    if completed.returncode < 0:
        # Killed by a signal; report it the way a shell would.
        return 128 - completed.returncode
    return completed.returncode


if __name__ == "__main__":
    sys.exit(main())
