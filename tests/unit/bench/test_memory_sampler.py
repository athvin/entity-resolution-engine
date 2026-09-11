"""Unit arm of the S10.4 memory sampler (ER-098): timing, the max selection, the
cgroup-absent fallback, the cursor-only DuckDB read, and the Compose envelope.

`benchmarks/` is a scripts directory, imported here the way the image imports it.
"""

from __future__ import annotations

import os
import re
import sys
import time
from pathlib import Path
from types import ModuleType
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[3]


def _import(module: str) -> ModuleType:
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


memory = _import("memory")
MemorySampler = memory.MemorySampler
MemoryPeaks = memory.MemoryPeaks


def test_sample_interval_and_sample_count() -> None:
    """AC2: 250 ms period; ≥6 samples over ~2 s; stop() joins fast and is idempotent."""
    assert memory.SAMPLE_INTERVAL_MS == 250

    sampler = MemorySampler(duckdb_source=lambda: 1, rss_source=lambda: 2, cgroup_source=lambda: 3)
    sampler.start()
    time.sleep(2.0)
    started = time.monotonic()
    peaks = sampler.stop()
    assert time.monotonic() - started < 1.0, "stop() did not join within its 1 s bound"
    assert sampler.sample_count >= 6, f"only {sampler.sample_count} samples over 2 s"
    assert isinstance(peaks, MemoryPeaks)
    # Idempotent: a second stop() neither raises nor changes the result.
    assert sampler.stop() == peaks


def test_peak_is_max_of_three_series() -> None:
    """AC3: memory_peak_bytes is the maximum of the three series; cgroup None drops out."""
    assert MemoryPeaks(100, 200, 300).memory_peak_bytes == 300
    assert MemoryPeaks(500, 200, 300).memory_peak_bytes == 500
    assert MemoryPeaks(100, 700, 300).memory_peak_bytes == 700
    # cgroup absent: the max is taken over the remaining two, not clamped to a 0 cgroup.
    assert MemoryPeaks(100, 200, None).memory_peak_bytes == 200

    # The running sampler keeps the per-series MAX across samples, not the last reading.
    # The sources clamp at their last value rather than raising StopIteration inside the
    # thread once the background loop outpaces the scripted readings.
    duckdb_series = [10, 90, 40]
    state = {"i": 0}

    def duckdb_src() -> int:
        value = duckdb_series[min(state["i"], len(duckdb_series) - 1)]
        state["i"] += 1
        return value

    sampler = MemorySampler(
        duckdb_source=duckdb_src,
        rss_source=lambda: 3,
        cgroup_source=lambda: 5,
        interval_ms=10,
    )
    sampler.start()
    time.sleep(0.1)
    peaks = sampler.stop()
    assert peaks.duckdb_memory_bytes == 90
    assert peaks.memory_peak_bytes == 90


def test_cgroup_peak_absent_yields_none(tmp_path: Path) -> None:
    """AC4: an absent memory.peak is None; the other two series are still sampled."""
    assert memory.read_cgroup_memory_peak(tmp_path / "does-not-exist") is None

    present = tmp_path / "memory.peak"
    present.write_text("4096\n", encoding="utf-8")
    assert memory.read_cgroup_memory_peak(present) == 4096
    present.write_text("max\n", encoding="utf-8")
    assert memory.read_cgroup_memory_peak(present) is None

    sampler = MemorySampler(
        duckdb_source=lambda: 111,
        rss_source=lambda: 222,
        cgroup_source=lambda: None,
        interval_ms=10,
    )
    sampler.start()
    time.sleep(0.05)
    peaks = sampler.stop()
    assert peaks.cgroup_peak_bytes is None
    assert peaks.duckdb_memory_bytes == 111 and peaks.rss_bytes == 222


class _SpyConnection:
    """A run connection that records whether a statement was executed ON IT."""

    def __init__(self) -> None:
        self.executed = False
        self.cursor_executed = False

    def execute(self, *_args: Any, **_kwargs: Any) -> Any:
        self.executed = True
        raise AssertionError("the sampler executed a statement on the run connection")

    def cursor(self) -> Any:
        outer = self

        class _Cursor:
            def execute(self, *_a: Any, **_k: Any) -> Any:
                outer.cursor_executed = True
                return self

            def fetchone(self) -> tuple[int]:
                return (12345,)

        return _Cursor()


def test_duckdb_memory_sampled_on_cursor_not_run_connection() -> None:
    """AC5: the DuckDB series reads a .cursor() duplicate, never the run connection."""
    connection = _SpyConnection()
    assert memory.current_duckdb_memory_bytes(connection) == 12345
    assert connection.cursor_executed, "the sampler did not use a cursor"
    assert not connection.executed, "the sampler executed on the run connection directly"


def test_compose_envelope_defaults() -> None:
    """AC7: Compose defaults 2 / 6g / 4GB, with ER_DUCKDB_THREADS derived from the quota.

    The strict arm reads `docker/compose.yaml` and pins the `${...:-default}` SYNTAX — the
    real AC7 check, and it runs in the host unit gate where the repo is present. The image
    does not copy `docker/` (it needs only `src/benchmarks/dbt/...` to run), so when the
    file is absent this asserts the envelope the defaults PRODUCED instead: with
    `ER_CPU_LIMIT` unset, Compose renders `ER_DUCKDB_THREADS=2` and the DuckDB memory limit
    to `4GB` in the container's environment. Both arms assert the AC7 values; neither skips.
    """
    compose = REPO_ROOT / "docker" / "compose.yaml"
    if compose.exists():
        text = compose.read_text(encoding="utf-8")
        # ER_DUCKDB_THREADS is the CPU quota itself, never an independent value.
        threads = re.search(r"ER_DUCKDB_THREADS:\s*\"\$\{ER_CPU_LIMIT:-2\}\"", text)
        assert threads, 'ER_DUCKDB_THREADS is not written as "${ER_CPU_LIMIT:-2}"'
        assert re.search(r"ER_DUCKDB_MEMORY_LIMIT:-4GB", text), (
            "the DuckDB memory default is not 4GB"
        )
        assert re.search(r"ER_CPU_LIMIT:-2", text), "the cpu quota default is not 2"
        assert re.search(r"ER_MEM_LIMIT:-6g", text), "the container memory default is not 6g"
    else:
        assert os.environ["ER_DUCKDB_THREADS"] == "2", (
            "ER_CPU_LIMIT is unset, so Compose should have rendered ER_DUCKDB_THREADS=2"
        )
        assert os.environ["ER_DUCKDB_MEMORY_LIMIT"] == "4GB", (
            "the container's DuckDB memory limit is not the 4GB default"
        )
