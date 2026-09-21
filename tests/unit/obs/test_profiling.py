from __future__ import annotations

import itertools
import json
import os
import sys
import uuid
from pathlib import Path
from types import SimpleNamespace

import duckdb
import pytest

from er.obs.profiling import ResourceSampler, span, stream_command
from er.obs.sql_profile import instrument_connection


def events(directory: Path) -> list[dict]:
    return [
        json.loads(line)
        for path in directory.glob("events-*.jsonl")
        for line in path.read_text().splitlines()
    ]


def test_nested_failure_and_zero_counts(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ER_PROFILE_DIR", str(tmp_path))
    with pytest.raises(ValueError), span("parent", run_id="test"):
        with span("child", unit="records") as counts:
            counts.update(rows_in=0, rows_out=0)
            raise ValueError("deliberate failure")
    records = events(tmp_path)
    starts = [record for record in records if record["event"] == "span_start"]
    ends = [record for record in records if record["event"] == "span_end"]
    assert starts[1]["parent_span_id"] == starts[0]["span_id"]
    assert all(record["status"] == "failed" and record["duration_ms"] >= 0 for record in ends)
    assert ends[0]["metrics"]["rows_in"] == 0


def test_profiles_survive_scalar_fetch_and_later_statements(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("ER_PROFILE_DIR", str(tmp_path))
    monkeypatch.setenv("ER_PROFILE_SQL", "1")
    connection = instrument_connection(duckdb.connect(":memory:"))
    with span("queries", run_id="test"):
        assert connection.execute("SELECT sum(i) FROM range(1000) t(i)").fetchone() == (499500,)
        connection.execute("CREATE TEMP TABLE probe AS SELECT i FROM range(10) t(i)")
        assert connection.sql("SELECT * FROM probe ORDER BY i").fetchall() == [
            (i,) for i in range(10)
        ]
        connection.execute("SELECT * FROM probe ORDER BY i")
        assert connection.fetchone() == (0,)
        assert connection.fetchmany(2) == [(1,), (2,)]
        assert connection.fetchall() == [(i,) for i in range(3, 10)]
    connection.close()
    profiles = [json.loads(path.read_text()) for path in (tmp_path / "sql").glob("*.json")]
    assert len(profiles) == 4
    assert all(profile["run_id"] == "test" for profile in profiles)
    assert all(profile["profile"]["latency"] > 0 for profile in profiles)
    assert not [event for event in events(tmp_path) if event["event"].endswith("_error")]


def test_disabled_profiling_keeps_native_connection(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("ER_PROFILE_DIR", raising=False)
    connection = duckdb.connect(":memory:")
    assert instrument_connection(connection) is connection
    connection.close()


def test_subprocess_streams_large_output_and_preserves_failure(tmp_path: Path) -> None:
    result = stream_command(
        [
            sys.executable,
            "-c",
            "import sys; print('x'*70000); print('failure', file=sys.stderr); sys.exit(7)",
        ],
        directory=tmp_path / "command",
    )
    assert result.returncode == 7
    assert len(result.stdout) <= 65536
    assert (tmp_path / "command/stdout.log").stat().st_size > 70000
    assert "failure" in (tmp_path / "command/stderr.log").read_text()


def test_resource_sampler_captures_child_memory(tmp_path: Path) -> None:
    sampler = ResourceSampler(tmp_path / "resources.jsonl", interval=0.02)
    sampler.start()
    try:
        result = stream_command(
            [sys.executable, "-c", "import time; block=bytearray(4000000); time.sleep(0.15)"],
            directory=tmp_path / "child",
        )
        assert result.returncode == 0
    finally:
        sampler.stop()
    assert sampler.error is None
    records = [json.loads(line) for line in sampler.destination.read_text().splitlines()]
    assert len(records) >= 2
    if Path("/proc/self/status").exists():
        assert any(len(record["process_rss_bytes"]) >= 2 for record in records)
        assert all(str(os.getpid()) in record["process_rss_bytes"] for record in records)


def test_sql_collection_failure_drains_pipe_and_is_reported(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("ER_PROFILE_DIR", str(tmp_path))
    monkeypatch.setenv("ER_PROFILE_SQL", "1")
    connection = instrument_connection(duckdb.connect(":memory:"))

    def fail(_profile: dict) -> None:
        raise OSError("simulated full disk")

    monkeypatch.setattr(connection, "_save", fail)
    for index in range(20):
        assert connection.execute("SELECT ?", [index]).fetchall() == [(index,)]
    connection.close()
    failures = [event for event in events(tmp_path) if event["event"] == "sql_profile_error"]
    assert len(failures) == 1
    assert "simulated full disk" in failures[0]["error_detail"]


def test_em_iteration_counter_counts_model_snapshots_not_parameter_rows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from er.matching.train import TrainCall, _invoke

    monkeypatch.setenv("ER_PROFILE_DIR", str(tmp_path))
    session = SimpleNamespace(
        _core_model_settings_history=[object(), object(), object()],
        _iteration_history_records=[{}] * 100,
    )
    linker = SimpleNamespace(training=SimpleNamespace(estimate=lambda: session))
    _invoke(linker, TrainCall("training.estimate", {}))
    ended = [event for event in events(tmp_path) if event["event"] == "span_end"]
    assert ended[0]["metrics"]["iterations"] == 2


def test_parameter_batch_profiles_use_full_unique_identifiers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("ER_PROFILE_DIR", str(tmp_path))
    monkeypatch.setenv("ER_PROFILE_SQL", "1")
    sequence = itertools.count(1)
    # All IDs share their first eight hex digits: truncation would overwrite files.
    monkeypatch.setattr(uuid, "uuid4", lambda: uuid.UUID(int=next(sequence)))
    connection = instrument_connection(duckdb.connect(":memory:"))
    connection.execute("CREATE TABLE unique_profiles (i INTEGER)")
    connection.executemany("INSERT INTO unique_profiles VALUES (?)", [[1], [2], [3]])
    connection.close()
    profiles = list((tmp_path / "sql").glob("*.json"))
    assert len(profiles) == 4
