"""One measured pass of S10.3's six phases, timed against the lake's own record (M24, M2).

`run_pass` runs the pipeline phase by phase — ingest, standardize, train, full match +
reconcile, assemble, then the incremental cycle — and returns a `PhaseRecord` per phase.
Three rules the spec states and a re-implementation gets wrong:

* **Timing is reconciled against `run_stages`, not taken from a second clock** (M2). The
  phase's wall time is measured once around its `er` invocations, and every other number
  — row counts, snapshot ranges — is read from the `run_stages` rows the stages wrote, so
  a phase's report and the lake's record cannot disagree. `wall_ms >= Σ duration_ms` is
  the reconciliation: wall includes process spawn, the stages' own sum does not.
* **`records_per_sec` divides by `wall_ms / 1000`** — dividing by `wall_ms` is off by
  1000 (records per millisecond), which is the defect the unit test pins.
* **`snapshot_count` is `Σ(snapshot_end − snapshot_start)` over the phase's stages** — a
  per-phase metric, never an assertion about how many snapshots a stage "should" commit
  (S4 preamble forbids that).

Corpus generation and `er init` run before measurement and are excluded from every
phase's time; the generated corpus is reused across repeat passes.

IO is behind the `Runner` protocol so the phase order and the arithmetic are unit-tested
without a lake; `BenchmarkRunner` is the production implementation the smoke run uses.
"""

from __future__ import annotations

import time
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Protocol

__all__ = [
    "PHASES",
    "PHASE_INGEST",
    "PHASE_STANDARDIZE",
    "PHASE_TRAIN",
    "PHASE_MATCH",
    "PHASE_ASSEMBLE",
    "PHASE_INCREMENTAL",
    "PhaseRecord",
    "Runner",
    "StageRow",
    "incremental_ratio",
    "phase_plan",
    "records_per_sec",
    "run_pass",
]

PHASE_INGEST = "ingest"
PHASE_STANDARDIZE = "standardize"
PHASE_TRAIN = "train"
PHASE_MATCH = "full_match_reconcile"
PHASE_ASSEMBLE = "assemble"
PHASE_INCREMENTAL = "incremental_cycle"

#: S10.3's six phases, in the order they must run and are reported.
PHASES: tuple[str, ...] = (
    PHASE_INGEST,
    PHASE_STANDARDIZE,
    PHASE_TRAIN,
    PHASE_MATCH,
    PHASE_ASSEMBLE,
    PHASE_INCREMENTAL,
)

#: The phases whose summed wall time is the denominator of `incremental_ratio`: the
#: full-pipeline cost the incremental cycle (phase 6) is compared against (S10.3).
_RATIO_DENOMINATOR = (PHASE_INGEST, PHASE_STANDARDIZE, PHASE_MATCH, PHASE_ASSEMBLE)

_SOURCES = ("crm", "billing", "webforms")


@dataclass(frozen=True)
class StageRow:
    """One `run_stages` row, narrowed to what a phase's metrics read."""

    stage: str
    duration_ms: int
    rows_in: int
    rows_out: int
    snapshot_start: int
    snapshot_end: int


@dataclass(frozen=True)
class PhaseRecord:
    """One phase's measured result (S10.3). `wall_ms` is the benchmark's measurement;
    the rest is read from the lake's own `run_stages` record for the phase."""

    name: str
    wall_ms: float
    rows_in: int
    rows_out: int
    records_per_sec: float
    candidate_pair_count: int
    pairs_above_auto_merge: int
    snapshot_count: int
    stage_duration_ms: int


def records_per_sec(rows_out: int, wall_ms: float) -> float:
    """`rows_out` per SECOND — divide by `wall_ms / 1000`, not by `wall_ms` (S10.3).

    Zero wall time yields ``0.0`` rather than raising: a phase fast enough to round to
    0 ms is a degenerate smoke-scale reading, not an error.
    """
    if wall_ms <= 0:
        return 0.0
    return rows_out / (wall_ms / 1000.0)


def incremental_ratio(records: Sequence[PhaseRecord]) -> float:
    """Phase 6 wall time ÷ the summed wall time of phases 1, 2, 4 and 5 (S10.3, G3).

    Raises:
        ValueError: a required phase is missing, or the denominator is zero.
    """
    by_name = {record.name: record for record in records}
    missing = [name for name in (*_RATIO_DENOMINATOR, PHASE_INCREMENTAL) if name not in by_name]
    if missing:
        raise ValueError(f"incremental_ratio is missing phase(s): {missing}")
    denominator = sum(by_name[name].wall_ms for name in _RATIO_DENOMINATOR)
    if denominator <= 0:
        raise ValueError("incremental_ratio denominator (phases 1+2+4+5) is zero")
    return by_name[PHASE_INCREMENTAL].wall_ms / denominator


class Runner(Protocol):
    """The IO a pass needs, behind a seam so the orchestration is testable (S8.4)."""

    def er(self, args: Sequence[str]) -> None:
        """Invoke the `er` CLI with `args`; raise on a non-success exit."""

    def stage_rows(self, run_id: str) -> list[StageRow]:
        """The `run_stages` rows written under `run_id`."""

    def candidate_pair_count(self) -> int:
        """DISTINCT canonical pairs sharing a blocking key, from `int_blocking_keys`."""

    def pairs_above_auto_merge(self, run_id: str) -> int:
        """`match_scores` rows at or above `auto_merge` this run scored."""


@dataclass(frozen=True)
class _PhaseSpec:
    """A phase: its reported name, the run_id it runs under, and its `er` invocations."""

    name: str
    run_id_key: str
    stages: tuple[str, ...]
    invocations: tuple[tuple[str, ...], ...]


def phase_plan(run_ids: dict[str, str]) -> list[_PhaseSpec]:
    """The six phases as `er` invocation sequences (S10.3), pure and order-fixed.

    `run_ids` maps the three run scopes — ``base``, ``train``, ``incremental`` — to ULIDs
    the caller minted. Phases 1, 2, 4 and 5 share the base run so reconcile's affected set
    is seeded by the base ingest; train and the incremental cycle are their own runs.
    """
    base, train, inc = run_ids["base"], run_ids["train"], run_ids["incremental"]
    ingest_base = tuple(("ingest", "--source", source, "--run-id", base) for source in _SOURCES)
    ingest_inc = tuple(("ingest", "--source", source, "--run-id", inc) for source in _SOURCES)
    return [
        _PhaseSpec(PHASE_INGEST, base, ("ingest",), ingest_base),
        _PhaseSpec(PHASE_STANDARDIZE, base, ("standardize",), (("standardize", "--run-id", base),)),
        _PhaseSpec(PHASE_TRAIN, train, ("train",), (("train", "--run-id", train),)),
        _PhaseSpec(
            PHASE_MATCH,
            base,
            ("match", "reconcile"),
            (("match", "--mode", "full", "--run-id", base), ("reconcile", "--run-id", base)),
        ),
        _PhaseSpec(PHASE_ASSEMBLE, base, ("assemble",), (("assemble", "--run-id", base),)),
        _PhaseSpec(
            PHASE_INCREMENTAL,
            inc,
            ("ingest", "standardize", "match", "reconcile", "assemble"),
            (
                *ingest_inc,
                ("standardize", "--changed-only", "--run-id", inc),
                ("match", "--mode", "incremental", "--run-id", inc),
                ("reconcile", "--run-id", inc),
                ("assemble", "--touched-only", "--run-id", inc),
            ),
        ),
    ]


def _measure_phase(spec: _PhaseSpec, runner: Runner, clock: Clock) -> PhaseRecord:
    started = clock()
    for invocation in spec.invocations:
        runner.er(invocation)
    wall_ms = (clock() - started) * 1000.0

    rows = [row for row in runner.stage_rows(spec.run_id_key) if row.stage in spec.stages]
    if not rows:
        raise ValueError(
            f"phase {spec.name!r} named no run_stages row under run_id {spec.run_id_key!r}; "
            "a phase's timing must reconcile against the lake's own record (M2)"
        )
    rows_in = sum(row.rows_in for row in rows)
    rows_out = sum(row.rows_out for row in rows)
    stage_duration_ms = sum(row.duration_ms for row in rows)
    snapshot_count = sum(row.snapshot_end - row.snapshot_start for row in rows)
    scored = "match" in spec.stages
    return PhaseRecord(
        name=spec.name,
        wall_ms=wall_ms,
        rows_in=rows_in,
        rows_out=rows_out,
        records_per_sec=records_per_sec(rows_out, wall_ms),
        candidate_pair_count=runner.candidate_pair_count() if scored else 0,
        pairs_above_auto_merge=runner.pairs_above_auto_merge(spec.run_id_key) if scored else 0,
        snapshot_count=snapshot_count,
        stage_duration_ms=stage_duration_ms,
    )


class Clock(Protocol):
    def __call__(self) -> float:
        """A monotonic seconds reading."""


def run_pass(
    runner: Runner,
    run_ids: dict[str, str],
    *,
    clock: Clock | None = None,
) -> list[PhaseRecord]:
    """Run the six phases in order and return a `PhaseRecord` for each.

    The caller has already generated the corpus and run `er init` — neither is timed
    here (S10.3). `clock` is injectable for tests; `time.monotonic` otherwise.
    """
    tick: Clock = time.monotonic if clock is None else clock
    return [_measure_phase(spec, runner, tick) for spec in phase_plan(run_ids)]
