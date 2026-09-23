"""Opt-in process diagnostics; uses the pinned interpreter's standard library."""

from __future__ import annotations

import cProfile
import os
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

from er.obs.profiling import emit, enabled


@contextmanager
def python_profile(component: str) -> Iterator[None]:
    if not enabled() or os.environ.get("ER_PROFILE_PYTHON") != "1":
        yield
        return
    root = Path(os.environ["ER_PROFILE_DIR"]) / "python"
    root.mkdir(parents=True, exist_ok=True)
    destination = root / f"{component}-{os.getpid()}-{uuid.uuid4().hex}.pstats"
    profiler = cProfile.Profile()
    profiler.enable()
    try:
        yield
    finally:
        profiler.disable()
        profiler.dump_stats(str(destination))
        emit("python_profile", component=component, profile_path=str(destination))
