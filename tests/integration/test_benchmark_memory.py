"""ER-098 integration: DuckDB is pinned to the S10.4 envelope on every connection, and
the memory sampler captures all three series during real DuckDB work.

DuckDB reads neither cgroup CPU nor cgroup memory, so `ER_DUCKDB_THREADS` /
`ER_DUCKDB_MEMORY_LIMIT` must be applied with `SET` on every connection the CLI, the
harness and dbt open (S7.1). `current_setting('memory_limit')` returns DuckDB's normalised
display form, so the expected value is normalised through a throwaway in-memory connection
rather than string-matched against the raw `4GB`.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path
from types import ModuleType
from typing import Any, Final

import duckdb
import pytest

from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR
from er.lake.ducklake import connect

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

SPEC_TEST_IDS: Final[tuple[str, ...]] = (
    "tests/integration/test_benchmark_memory.py::test_every_connection_pins_threads_and_memory",
)


def _import(module: str) -> ModuleType:
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


memory = _import("memory")


def _expected_threads() -> int:
    return int(os.environ["ER_DUCKDB_THREADS"])


def _normalized_memory_limit(raw: str) -> str:
    """`raw` as DuckDB's own `current_setting('memory_limit')` display form.

    `SET memory_limit='4GB'` reads back as `'3.7 GiB'`; normalising the expected value
    through the same engine is what makes the comparison exact without hard-coding that
    rounding here.
    """
    mem = duckdb.connect(":memory:")
    mem.execute(f"SET memory_limit = '{raw}'")
    row = mem.execute("SELECT current_setting('memory_limit')").fetchone()
    assert row is not None
    return str(row[0])


def _assert_pinned(threads: Any, memory_limit: Any, *, where: str) -> None:
    expected_mem = _normalized_memory_limit(os.environ["ER_DUCKDB_MEMORY_LIMIT"])
    assert int(threads) == _expected_threads(), (
        f"{where}: threads is {threads!r}, env ER_DUCKDB_THREADS is {_expected_threads()}"
    )
    assert str(memory_limit) == expected_mem, (
        f"{where}: memory_limit is {memory_limit!r}, expected {expected_mem!r} "
        f"(normalised from ER_DUCKDB_MEMORY_LIMIT={os.environ['ER_DUCKDB_MEMORY_LIMIT']!r})"
    )


def _settings_of(connection: duckdb.DuckDBPyConnection) -> tuple[Any, Any]:
    threads = connection.execute("SELECT current_setting('threads')").fetchone()
    mem = connection.execute("SELECT current_setting('memory_limit')").fetchone()
    assert threads is not None and mem is not None
    return threads[0], mem[0]


def _dbt_settings() -> tuple[Any, Any]:
    """Read `threads`/`memory_limit` off the connection dbt opens against the lake target."""
    result = subprocess.run(
        [
            "dbt",
            "show",
            "--inline",
            "SELECT current_setting('threads') AS threads, "
            "current_setting('memory_limit') AS memory_limit",
            "--target",
            "lake",
            "--project-dir",
            DBT_PROJECT_DIR,
            "--profiles-dir",
            DBT_PROFILES_DIR,
            "--output",
            "json",
        ],
        capture_output=True,
        text=True,
        env=dict(os.environ),
        check=False,
    )
    assert result.returncode == 0, f"dbt show failed:\n{result.stdout}\n{result.stderr}"
    decoder = json.JSONDecoder()
    brace = result.stdout.index("{")
    document, _ = decoder.raw_decode(result.stdout[brace:])
    row = document["show"][0]
    return row["threads"], row["memory_limit"]


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored — `dbt show` below parses the
    project, which references dbt_utils, so the packages must be present in the image."""
    if (Path(DBT_PROJECT_DIR) / "dbt_packages" / "dbt_utils").is_dir():
        return
    completed = subprocess.run(
        ["dbt", "deps", "--project-dir", DBT_PROJECT_DIR, "--profiles-dir", DBT_PROFILES_DIR],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


def test_every_connection_pins_threads_and_memory(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC6: the harness, the CLI's `connect()`, and dbt's `lake` target all pin threads
    and memory_limit to the scale's envelope (S7.1) — not to DuckDB's host-derived default."""
    harness_threads, harness_mem = _settings_of(initialised_lake)
    _assert_pinned(harness_threads, harness_mem, where="harness connection")

    # The CLI opens lake connections through `connect()`; asserting it covers every `er`.
    with connect() as cli_connection:
        cli_threads, cli_mem = _settings_of(cli_connection)
    _assert_pinned(cli_threads, cli_mem, where="CLI connect()")

    dbt_threads, dbt_mem = _dbt_settings()
    _assert_pinned(dbt_threads, dbt_mem, where="dbt lake target")


def test_phase_records_carry_three_memory_series(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC3 (live): while real DuckDB work runs, the sampler records all three series and
    reports the per-series peaks, with memory_peak_bytes the maximum across them."""
    connection = initialised_lake
    sampler = memory.MemorySampler(
        duckdb_source=lambda: memory.current_duckdb_memory_bytes(connection),
        interval_ms=50,
    )
    sampler.start()
    # A materialised workload that actually holds buffer memory while the sampler reads it
    # off a cursor duplicate (never off this connection).
    connection.execute(
        "CREATE OR REPLACE TEMP TABLE _er_mem_probe AS "
        "SELECT i, hash(i) AS h FROM range(1000000) AS t(i)"
    )
    time.sleep(0.2)
    connection.execute("DROP TABLE _er_mem_probe")
    peaks = sampler.stop()

    assert sampler.sample_count >= 1
    assert peaks.duckdb_memory_bytes >= 0
    assert peaks.rss_bytes >= 0
    assert peaks.cgroup_peak_bytes is None or isinstance(peaks.cgroup_peak_bytes, int)
    expected_max = max(peaks.duckdb_memory_bytes, peaks.rss_bytes, peaks.cgroup_peak_bytes or 0)
    assert peaks.memory_peak_bytes == expected_max
    assert peaks.memory_peak_bytes > 0, "no series recorded any memory during real work"
