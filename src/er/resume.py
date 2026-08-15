"""`er run-all --resume <run_id>`: the S4.7 recovery decision (DesignDoc.md S4.7).

S4.7 states the whole contract in one paragraph: resume "reads `run_stages` for that
run, identifies the first stage whose `status` is not `succeeded`, and restarts the
chain from there with the original `run_id`, `config_hash` and `model_version`. It
refuses (exit `2`) if the config on disk hashes differently from the `runs.config_hash`
of the run being resumed, or (exit `3`) if the run is already `succeeded`. Resume
re-executes the failed stage in full; it never resumes mid-stage."

Three things are structural here:

* **The decision is a pure function.** :func:`resume_plan` takes rows and a hash and
  returns a plan or raises. Nothing in it opens a connection, so the refusals — which
  are the part an operator meets at the worst possible moment — are testable without a
  lake, and :func:`read_resume_rows` is the only impure half.
* **The unit of restart is a whole stage.** The plan names a stage, never an offset
  into one. S4.7 forbids resuming mid-stage, and the reason it can afford to is the S4
  preamble's idempotence rule: re-executing a failed stage over the same inputs is
  always safe.
* **Nothing here mints anything.** The `run_id` comes off the row and the `config_hash`
  is the one the run was recorded under, not the one just computed — that second value
  exists only to be compared against. A resume that minted a new `run_id` would leave
  the failed run unresumable and its snapshot ranges unaddressable.

The config-drift guard on `(config_hash, model_version, std_version)` and
`--allow-escalate` are a different refusal, on a different comparison, and belong to
ER-034. This module compares one run against its own recorded hash and nothing else.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from typing import Final

import duckdb

from er.errors import ConfigError, PreconditionFailure
from er.lake.model import SCHEMA_QUALIFIER
from er.obs.runctx import STATUS_SUCCEEDED

__all__ = [
    "ResumePlan",
    "ResumeRow",
    "read_resume_rows",
    "resume_plan",
]

_RUNS: Final[str] = "runs"
_RUN_STAGES: Final[str] = "run_stages"


@dataclass(frozen=True)
class ResumeRow:
    """One `run_stages` row of the run being resumed, joined to its `runs` row.

    The join is carried in the row rather than passed alongside it because every
    field below is a property of the *run*, and a plan built from stage rows plus a
    separately-fetched run row could be built from two different runs.
    """

    run_id: str
    mode: str
    run_status: str
    config_hash: str
    model_version: str | None
    stage: str
    seq: int
    status: str


@dataclass(frozen=True)
class ResumePlan:
    """What `--resume` will do: where to restart, and what to restart it as.

    ``config_hash`` and ``model_version`` are the run's own, so every stage the resume
    executes is recorded under the values the stages before it were recorded under.
    """

    run_id: str
    mode: str
    config_hash: str
    model_version: str | None
    #: The first stage whose `status` is not `succeeded` — the chain restarts here.
    resume_from: str
    #: The stages already `succeeded`, in `seq` order. They are not re-executed, and
    #: their rows — `started_at` included — are left exactly as the failed run wrote
    #: them.
    completed: tuple[str, ...]


def resume_plan(rows: Sequence[ResumeRow], on_disk_config_hash: str) -> ResumePlan:
    """The S4.7 plan for resuming the run ``rows`` describe.

    Args:
        rows: the run's `run_stages` rows joined to its `runs` row, in any order.
        on_disk_config_hash: the `config_hash` of the document this process just
            validated, which is the only thing it is compared against.

    Raises:
        er.errors.ConfigError: the config on disk hashes differently from the run's
            (exit `2`). Checked first, in S4.7's own order: a run whose config moved
            cannot be restarted under either status, and reporting "already succeeded"
            for it would send the operator after the wrong fault.
        er.errors.PreconditionFailure: there is nothing to resume (exit `3`) — no
            recorded stage, an already-`succeeded` run, or a run whose stages all
            succeeded and which therefore has no stage to restart from.
    """
    if not rows:
        raise PreconditionFailure(
            "no run_stages rows for the run being resumed: there is nothing to restart from"
        )

    ordered = sorted(rows, key=lambda row: row.seq)
    first = ordered[0]
    _assert_one_run(ordered)

    if first.config_hash != on_disk_config_hash:
        raise ConfigError(
            f"config_hash mismatch: run {first.run_id} was recorded under "
            f"{first.config_hash}, the config on disk hashes to {on_disk_config_hash}"
        )
    if first.run_status == STATUS_SUCCEEDED:
        raise PreconditionFailure(f"run {first.run_id} already succeeded: nothing to resume")

    unfinished = [row for row in ordered if row.status != STATUS_SUCCEEDED]
    if not unfinished:
        # `runs.status` says the run failed, but every stage it recorded succeeded --
        # the run died between its last stage and its own terminal write. There is no
        # stage to re-execute, and inventing one (the next in the chain) would be this
        # module guessing at a chain it is deliberately not given.
        raise PreconditionFailure(
            f"run {first.run_id} has no unfinished stage: every recorded stage succeeded"
        )

    return ResumePlan(
        run_id=first.run_id,
        mode=first.mode,
        config_hash=first.config_hash,
        model_version=first.model_version,
        resume_from=unfinished[0].stage,
        completed=tuple(row.stage for row in ordered if row.status == STATUS_SUCCEEDED),
    )


def _assert_one_run(rows: Sequence[ResumeRow]) -> None:
    """Refuse rows that do not all describe one run.

    A caller that merged two runs' rows would otherwise get a plan naming one run's
    `run_id` and another's failed stage, and would re-execute it under the wrong id.
    """
    identifiers = {row.run_id for row in rows}
    if len(identifiers) != 1:
        raise PreconditionFailure(f"rows describe more than one run: {sorted(identifiers)}")
    stages = [row.stage for row in rows]
    if len(set(stages)) != len(stages):
        # S5.2 keys `run_stages` on `(run_id, stage)`; a duplicate here means the key
        # has already been broken, and resuming from a run whose ledger is ambiguous
        # would write over the wrong row.
        raise PreconditionFailure(f"run {rows[0].run_id} has more than one row for a stage")


def read_resume_rows(connection: duckdb.DuckDBPyConnection, run_id: str) -> tuple[ResumeRow, ...]:
    """Load the rows :func:`resume_plan` decides from, in `seq` order.

    An empty result means the run has no recorded stage — because it does not exist,
    or because it died before its first stage wrote a row. Both are the same refusal
    and :func:`resume_plan` raises it; distinguishing them would not change what an
    operator does next.
    """
    rows = connection.execute(
        f"SELECT run.run_id, run.mode, run.status, run.config_hash, run.model_version, "
        f"stage.stage, stage.seq, stage.status "
        f"FROM {SCHEMA_QUALIFIER}.{_RUNS} AS run "
        f"JOIN {SCHEMA_QUALIFIER}.{_RUN_STAGES} AS stage ON stage.run_id = run.run_id "
        f"WHERE run.run_id = ? ORDER BY stage.seq",
        [run_id],
    ).fetchall()
    return tuple(
        ResumeRow(
            run_id=str(row[0]),
            mode=str(row[1]),
            run_status=str(row[2]),
            config_hash=str(row[3]),
            model_version=None if row[4] is None else str(row[4]),
            stage=str(row[5]),
            seq=int(row[6]),
            status=str(row[7]),
        )
        for row in rows
    )
