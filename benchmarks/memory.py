"""The S10.4 memory sampler: peak memory over a measured phase, three ways (M24, M25).

`duckdb_memory()` reports the buffer manager's CURRENT usage with no peak counter, and
excludes vectors, query results, the Python heap and the dbt subprocess — so a single
reading at phase end is near zero and useless for pod sizing. This module samples three
series every 250 ms in a background thread and keeps each one's maximum:

* **DuckDB buffer memory** — `sum(memory_usage_bytes)` from `duckdb_memory()`, sampled on
  a `.cursor()` duplicate of the run connection so the sampler never serialises behind,
  or perturbs, the work being measured.
* **Process RSS** — `/proc/self/status` `VmRSS`, the resident set of the whole process.
* **cgroup peak** — `/sys/fs/cgroup/memory.peak`, the only source that includes DuckDB's
  out-of-buffer allocations, the Python heap AND the dbt subprocess, so it is the number
  S10.4 feeds to sizing even though it is the least precise about which component grew.
  Absent (cgroup v1, or not containerised) it is `None`, recorded as null rather than 0.

The sources are injectable so the thread's timing and the max selection are unit-tested
without a lake or a container.
"""

from __future__ import annotations

import threading
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final

__all__ = [
    "SAMPLE_INTERVAL_MS",
    "MemoryPeaks",
    "MemorySampler",
    "current_duckdb_memory_bytes",
    "read_cgroup_memory_peak",
]

#: The sampling period S10.4 fixes. 250 ms is frequent enough to catch a phase's peak
#: and rare enough not to perturb it.
SAMPLE_INTERVAL_MS: Final = 250

_CGROUP_MEMORY_PEAK = Path("/sys/fs/cgroup/memory.peak")
_PROC_STATUS = Path("/proc/self/status")
_UNLIMITED = "max"


@dataclass(frozen=True)
class MemoryPeaks:
    """The peak of each sampled series over a phase. `cgroup_peak_bytes` is ``None`` when
    the cgroup v2 `memory.peak` file is absent (S10.4)."""

    duckdb_memory_bytes: int
    rss_bytes: int
    cgroup_peak_bytes: int | None

    @property
    def memory_peak_bytes(self) -> int:
        """The maximum across the three series — the single pod-sizing number (S10.4).

        `cgroup_peak_bytes` is the authoritative series when present (it alone counts
        out-of-buffer and subprocess memory), so it usually IS the maximum; when it is
        absent it simply drops out of the max rather than counting as zero.
        """
        return max(self.duckdb_memory_bytes, self.rss_bytes, self.cgroup_peak_bytes or 0)


def read_cgroup_memory_peak(path: Path = _CGROUP_MEMORY_PEAK) -> int | None:
    """cgroup v2 `memory.peak` in bytes, or ``None`` when absent or unlimited (S10.4).

    A missing file (cgroup v1, or not containerised) is `None`, not an error: the series
    is genuinely unavailable there, and the result records that as null.
    """
    try:
        text = Path(path).read_text(encoding="utf-8")
    except OSError:
        return None
    stripped = text.strip()
    return None if stripped in ("", _UNLIMITED) else int(stripped)


def current_duckdb_memory_bytes(connection: Any) -> int:
    """`sum(memory_usage_bytes)` from `duckdb_memory()`, on a CURSOR of `connection`.

    The cursor duplicate is the point (S10.4, S4.0b): the sampler must read memory without
    taking the run connection's lock or interleaving a statement into the work being
    measured, so it never touches `connection` directly — only a cursor off it.
    """
    cursor = connection.cursor()
    row = cursor.execute(
        "SELECT coalesce(sum(memory_usage_bytes), 0) FROM duckdb_memory()"
    ).fetchone()
    return int(row[0]) if row is not None and row[0] is not None else 0


def _current_rss_bytes(status_path: Path = _PROC_STATUS) -> int:
    """Resident set size from `/proc/self/status` `VmRSS`; 0 where `/proc` is absent."""
    try:
        text = status_path.read_text(encoding="utf-8")
    except OSError:
        return 0
    for line in text.splitlines():
        if line.startswith("VmRSS:"):
            parts = line.split()
            # `VmRSS:  <kB> kB`
            return int(parts[1]) * 1024 if len(parts) >= 2 else 0
    return 0


class MemorySampler:
    """A background thread sampling three memory series every `interval_ms` (S10.4).

    `start()` takes one immediate sample and then one every interval; `stop()` ends the
    thread, joins it within a bound, and returns the per-series peaks. `stop()` is
    idempotent. The sources are callables so the thread is testable with synthetic series.
    """

    def __init__(
        self,
        *,
        duckdb_source: Callable[[], int],
        rss_source: Callable[[], int] = _current_rss_bytes,
        cgroup_source: Callable[[], int | None] = read_cgroup_memory_peak,
        interval_ms: int = SAMPLE_INTERVAL_MS,
    ) -> None:
        self._duckdb_source = duckdb_source
        self._rss_source = rss_source
        self._cgroup_source = cgroup_source
        self._interval = interval_ms / 1000.0
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._lock = threading.Lock()
        self._peak_duckdb = 0
        self._peak_rss = 0
        self._peak_cgroup: int | None = None
        self._sample_count = 0

    @property
    def sample_count(self) -> int:
        with self._lock:
            return self._sample_count

    def _sample(self) -> None:
        duckdb_bytes = self._duckdb_source()
        rss_bytes = self._rss_source()
        cgroup_bytes = self._cgroup_source()
        with self._lock:
            self._peak_duckdb = max(self._peak_duckdb, duckdb_bytes)
            self._peak_rss = max(self._peak_rss, rss_bytes)
            if cgroup_bytes is not None:
                self._peak_cgroup = (
                    cgroup_bytes
                    if self._peak_cgroup is None
                    else max(self._peak_cgroup, cgroup_bytes)
                )
            self._sample_count += 1

    def _run(self) -> None:
        self._sample()  # one reading immediately, so a phase shorter than one interval still counts
        while not self._stop.wait(self._interval):
            self._sample()

    def start(self) -> None:
        if self._thread is not None:
            raise RuntimeError("MemorySampler.start called twice")
        self._thread = threading.Thread(target=self._run, name="er-memory-sampler", daemon=True)
        self._thread.start()

    def stop(self) -> MemoryPeaks:
        self._stop.set()
        thread = self._thread
        if thread is not None:
            thread.join(timeout=1.0)
            self._thread = None
        with self._lock:
            return MemoryPeaks(
                duckdb_memory_bytes=self._peak_duckdb,
                rss_bytes=self._peak_rss,
                cgroup_peak_bytes=self._peak_cgroup,
            )
