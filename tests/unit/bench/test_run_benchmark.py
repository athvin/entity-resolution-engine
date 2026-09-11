"""Unit arm of Benchmark B (S10.3, S10.4): phase order, the two metric formulas, and the
cgroup CPU parser — the logic a container cannot isolate and that a weaker test gets off
by a factor of 1000.

`benchmarks/` is a directory of scripts, not a distribution, so its modules import each
other by bare name (`from schema import ...`). The same `sys.path` entry the image and
`python benchmarks/run_benchmark.py` rely on is added here before importing them.
"""

from __future__ import annotations

import sys
from collections.abc import Sequence
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]


def _import(module: str) -> ModuleType:
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


run_benchmark = _import("run_benchmark")
fingerprint = _import("fingerprint")

PhaseRecord = run_benchmark.PhaseRecord
StageRow = run_benchmark.StageRow


class _SpyRunner:
    """Records every `er` invocation and answers with synthetic lake reads."""

    _STAGES = ("ingest", "standardize", "train", "match", "reconcile", "assemble")

    def __init__(self) -> None:
        self.calls: list[tuple[str, ...]] = []

    def er(self, args: Sequence[str]) -> None:
        self.calls.append(tuple(args))

    def stage_rows(self, run_id: str) -> list[StageRow]:
        return [
            StageRow(
                stage=stage,
                duration_ms=5,
                rows_in=1000,
                rows_out=1000,
                snapshot_start=1,
                snapshot_end=3,
            )
            for stage in self._STAGES
        ]

    def candidate_pair_count(self) -> int:
        return 42

    def pairs_above_auto_merge(self, run_id: str) -> int:
        return 7


class _Clock:
    """A deterministic monotonic clock: each read advances by one second."""

    def __init__(self) -> None:
        self._t = 0.0

    def __call__(self) -> float:
        self._t += 1.0
        return self._t


def test_phase_order_and_names() -> None:
    """AC2: the six phases run in S10.3 order and the underlying `er` verb sequence is
    ingest, standardize, train, match --mode full + reconcile, assemble, incremental."""
    runner = _SpyRunner()
    run_ids = {"base": "RB", "train": "RT", "incremental": "RI"}
    records = run_benchmark.run_pass(runner, run_ids, clock=_Clock())

    assert [record.name for record in records] == list(run_benchmark.PHASES)

    verbs = [call[0] for call in runner.calls]
    assert verbs == [
        "ingest",
        "ingest",
        "ingest",  # phase 1, one per source
        "standardize",  # phase 2
        "train",  # phase 3
        "match",
        "reconcile",  # phase 4
        "assemble",  # phase 5
        "ingest",
        "ingest",
        "ingest",
        "standardize",
        "match",
        "reconcile",
        "assemble",  # phase 6
    ]
    # The full-match phase carries the one `--mode full`; the cycle carries `incremental`.
    full = next(
        call
        for call in runner.calls
        if call[0] == "match" and "--run-id" in call and call[call.index("--run-id") + 1] == "RB"
    )
    assert "full" in full
    cycle = next(
        call
        for call in runner.calls
        if call[0] == "match" and call[call.index("--run-id") + 1] == "RI"
    )
    assert "incremental" in cycle


def test_records_per_sec_uses_seconds() -> None:
    """AC4: 1000 rows in 2000 ms is 500 rows/sec, not 0.5 (divide by wall_ms/1000)."""
    assert run_benchmark.records_per_sec(1000, 2000) == 500.0
    assert run_benchmark.records_per_sec(0, 2000) == 0.0
    assert run_benchmark.records_per_sec(1000, 0) == 0.0


def _record(name: str, wall_ms: float) -> Any:
    return PhaseRecord(
        name=name,
        wall_ms=wall_ms,
        rows_in=0,
        rows_out=0,
        records_per_sec=0.0,
        candidate_pair_count=0,
        pairs_above_auto_merge=0,
        snapshot_count=0,
        stage_duration_ms=0,
    )


def test_incremental_ratio_formula() -> None:
    """AC4: incremental_ratio is phase 6 wall ÷ (phases 1+2+4+5) wall, exactly."""
    records = [
        _record(run_benchmark.PHASE_INGEST, 100.0),
        _record(run_benchmark.PHASE_STANDARDIZE, 200.0),
        _record(run_benchmark.PHASE_TRAIN, 9999.0),  # phase 3 is NOT in the denominator
        _record(run_benchmark.PHASE_MATCH, 300.0),
        _record(run_benchmark.PHASE_ASSEMBLE, 400.0),
        _record(run_benchmark.PHASE_INCREMENTAL, 500.0),
    ]
    # 500 / (100 + 200 + 300 + 400) == 0.5, and train's 9999 is excluded.
    assert run_benchmark.incremental_ratio(records) == 0.5

    with pytest.raises(ValueError):
        run_benchmark.incremental_ratio(records[:-1])  # no incremental phase


def test_cgroup_cpu_max_parsing() -> None:
    """AC8: cpu.max is quota ÷ period — never a core count; `max` is unlimited (None)."""
    assert fingerprint.read_cgroup_cpu_max("200000 100000") == 2.0
    assert fingerprint.read_cgroup_cpu_max("100000 100000") == 1.0
    assert fingerprint.read_cgroup_cpu_max("50000 100000") == 0.5
    assert fingerprint.read_cgroup_cpu_max("max 100000") is None
    with pytest.raises(ValueError):
        fingerprint.read_cgroup_cpu_max("200000")
    with pytest.raises(ValueError):
        fingerprint.read_cgroup_cpu_max("200000 0")
