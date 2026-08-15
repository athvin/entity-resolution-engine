"""`runs` and `run_stages`: the referent of every `run_id` (DesignDoc.md S4, S5.2).

Six relations carry a `run_id` and, until this module, none of them pointed at
anything. :class:`RunContext` is the one writer of the `runs` row and
:meth:`RunContext.stage` the one writer of a `run_stages` row, so "one row per CLI
invocation" and "exactly one row per stage per run" are properties of the control
flow rather than conventions each stage follows.

Five rules are structural here, and each is a decision the spec makes:

* **A stage records a snapshot RANGE, never a count.** A stage may commit many
  snapshots (S4 preamble), and a no-op run may legitimately commit none, so
  ``(snapshot_start, snapshot_end)`` is captured around the stage body and nothing
  anywhere counts snapshots.
* **The terminal state is written on every exit path.** A stage whose body raises
  still gets ``status='failed'``, ``ended_at``, its partial range and a non-NULL
  ``error_class``; the run it belongs to becomes ``status='failed'`` (S4.7). A
  ``run_stages`` row left ``running`` would be indistinguishable from a live stage.
* **A connection is opened per write, never held across a stage body.** S4.0b:
  "No Python DuckDB connection spans a dbt invocation", and a dbt-backed stage is
  precisely what runs between a stage's entry and exit writes.
* **`(run_id, stage)` holds one row.** S5.2 keys `run_stages` on both `(run_id, seq)`
  and `(run_id, stage)`, so re-executing a stage — which `--resume` (ER-024) does —
  updates its row and reuses its `seq` instead of appending a second one.
* **Telemetry is unconditional; persistence is not.** The S5.2 stderr record is
  emitted for every stage, always. The rows are written only when there is a lake
  to write them to: a namespace whose relations `er init` has not yet created, or
  an invocation with no validated config (`runs` requires `tenant`, `config_hash`,
  `std_version` and `survivorship_version`, all of which come from the document),
  persists nothing rather than failing the run over its own bookkeeping. Only
  :class:`~er.lake.env.MissingEnvError` is treated that way; a write that fails
  once persistence is on is a real fault and propagates.

Out of scope by construction: the S4.7 *values* of `error_class` and the advisory
lock and ``--resume`` (both ER-024). This module persists whatever class the raised
error carries and never leaves it NULL on a failure.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable, Iterator, Mapping
from contextlib import AbstractContextManager, contextmanager, nullcontext
from dataclasses import dataclass, field
from datetime import UTC, datetime
from typing import Final, TextIO

import duckdb

from er.errors import ErrorClass, ExitCode
from er.lake.ducklake import LAKE_ALIAS, current_snapshot
from er.lake.env import MissingEnvError
from er.lake.model import REGISTRY, RUN_MODES, RUN_STAGES, SCHEMA_QUALIFIER
from er.obs.counters import DECLARED_COUNTERS, PROMOTED_COUNTERS, StageCounters
from er.obs.logging import emit_stage_record
from er.versions import code_version

__all__ = [
    "STATUS_FAILED",
    "STATUS_RUNNING",
    "STATUS_SUCCEEDED",
    "ConnectionSource",
    "RunContext",
    "StageRun",
    "held",
]

#: `runs.status` and `run_stages.status` share one vocabulary (S5). A stage that
#: exits ``10`` is SUCCEEDED: S4.0 makes "nothing to do" a successful no-op, which
#: is why the record also carries `exit_code`.
STATUS_RUNNING: Final[str] = "running"
STATUS_SUCCEEDED: Final[str] = "succeeded"
STATUS_FAILED: Final[str] = "failed"

#: The exit codes that leave a stage `succeeded` (S4.0).
_SUCCESS_EXITS: Final[frozenset[int]] = frozenset(
    {int(ExitCode.SUCCESS), int(ExitCode.NOTHING_TO_DO)}
)

_RUNS: Final[str] = "runs"
_RUN_STAGES: Final[str] = "run_stages"

#: A source of lake connections: called once per write, entered, and closed again.
#: The CLI passes :func:`er.lake.ducklake.connect` itself; a caller that already
#: holds a connection passes :func:`held`.
ConnectionSource = Callable[[], AbstractContextManager[duckdb.DuckDBPyConnection]]


def held(connection: duckdb.DuckDBPyConnection) -> ConnectionSource:
    """A :data:`ConnectionSource` that reuses ``connection`` and never closes it.

    For a caller that already has the lake attached — the S8.1 test harness, and
    every in-process stage that opens its own connection for its own work. Handing
    the same connection back is what stops a second attachment from being opened
    for the bookkeeping of a stage that is already connected.
    """
    return lambda: nullcontext(connection)


def _now() -> datetime:
    """The instant a timestamp column and a log line both take."""
    return datetime.now(UTC)


def _iso(moment: datetime | None) -> str | None:
    """``moment`` as the ISO-8601 UTC instant the S5.2 record carries."""
    if moment is None:
        return None
    return moment.isoformat(timespec="milliseconds").replace("+00:00", "Z")


def _sql_timestamp(moment: datetime | None) -> datetime | None:
    """``moment`` as a naive UTC value, for a `TIMESTAMP` column.

    S5's timestamp columns are `TIMESTAMP`, not `TIMESTAMPTZ` (S5.0 lists eight
    permitted types and only one of them is a timestamp), so the offset is dropped
    here — once, at the boundary — rather than left to an implicit engine cast.
    """
    if moment is None:
        return None
    return moment.astimezone(UTC).replace(tzinfo=None)


def _write_row(
    connection: duckdb.DuckDBPyConnection, table: str, values: Mapping[str, object]
) -> None:
    """`INSERT` one complete row into a registry relation.

    ``values`` must name every column of the spec: the registry is the column list
    (S5), so a partial mapping is a caller that has drifted from it, and DuckLake
    has no `DEFAULT` (B2) to fill the gap with.
    """
    columns = REGISTRY[table].column_names
    supplied = set(values)
    if supplied != set(columns):
        raise ValueError(
            f"{table}: row does not match the registry; "
            f"missing={sorted(set(columns) - supplied)}, extra={sorted(supplied - set(columns))}"
        )
    placeholders = ",".join("?" for _ in columns)
    connection.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.{table} ({','.join(columns)}) VALUES ({placeholders})",
        [values[column] for column in columns],
    )


def _update_row(
    connection: duckdb.DuckDBPyConnection,
    table: str,
    values: Mapping[str, object],
    where: Mapping[str, object],
) -> None:
    """`UPDATE` the columns in ``values`` for the rows matching ``where``."""
    assignments = ",".join(f"{column}=?" for column in values)
    predicate = " AND ".join(f"{column}=?" for column in where)
    connection.execute(
        f"UPDATE {SCHEMA_QUALIFIER}.{table} SET {assignments} WHERE {predicate}",
        [*values.values(), *where.values()],
    )


def _relations_present(connection: duckdb.DuckDBPyConnection) -> bool:
    """Whether this namespace has been through `er init` yet.

    The two relations are created by `er init` and by nothing else (S7.1), so their
    absence means the lake is not initialised — which is the state `er init` itself
    runs in, and not a failure of the invocation that noticed it.
    """
    row = connection.execute(
        "SELECT count(*) FROM duckdb_tables() WHERE database_name = ? AND table_name IN (?, ?)",
        [LAKE_ALIAS, _RUNS, _RUN_STAGES],
    ).fetchone()
    return row is not None and int(row[0]) == 2


@dataclass
class StageRun:
    """One stage of one run: the `run_stages` row and the S5.2 record it emits.

    Mutable on purpose. The row is written twice — ``running`` on entry and its
    terminal state on exit — and this object is what the caller fills in between,
    through :attr:`counters` and :meth:`finish`.
    """

    run_id: str
    stage: str
    seq: int
    started_at: datetime
    counters: StageCounters
    config_hash: str | None = None
    model_version: str | None = None
    tf_snapshot_id: str | None = None
    status: str = STATUS_RUNNING
    exit_code: int | None = None
    ended_at: datetime | None = None
    snapshot_start: int | None = None
    snapshot_end: int | None = None
    error_class: ErrorClass | None = None
    error_detail: str | None = None

    def finish(
        self,
        exit_code: int,
        *,
        error_class: ErrorClass | None = None,
        error_detail: str | None = None,
    ) -> None:
        """Record the terminal state of the stage.

        ``status`` is derived from ``exit_code`` and never chosen: S4.0 makes ``10``
        a successful no-op, so a stage that exits ``10`` is ``succeeded`` with a NULL
        ``error_class`` and one that exits ``1`` is ``failed``.

        A failure with no class is classified ``data``, which is the default class of
        :class:`~er.errors.StageFailure`: S4.7 owns the taxonomy (ER-024), but a
        ``failed`` row with a NULL ``error_class`` tells an operator nothing at all.
        """
        self.exit_code = exit_code
        self.status = STATUS_SUCCEEDED if exit_code in _SUCCESS_EXITS else STATUS_FAILED
        if self.status == STATUS_FAILED:
            self.error_class = error_class if error_class is not None else ErrorClass.DATA
            self.error_detail = error_detail
        else:
            self.error_class = None
            self.error_detail = None

    @property
    def duration_ms(self) -> int | None:
        """Wall time of the stage, the one promoted counter the stage never supplies."""
        if self.ended_at is None:
            return None
        return int((self.ended_at - self.started_at).total_seconds() * 1000)

    def record(self) -> dict[str, object]:
        """The S5.2 record, with the key set :data:`~er.obs.logging.STAGE_RECORD_KEYS`."""
        return {
            "run_id": self.run_id,
            "stage": self.stage,
            "seq": self.seq,
            "status": self.status,
            "exit_code": self.exit_code,
            "started_at": _iso(self.started_at),
            "ended_at": _iso(self.ended_at),
            "snapshot_start": self.snapshot_start,
            "snapshot_end": self.snapshot_end,
            "config_hash": self.config_hash,
            "model_version": self.model_version,
            "tf_snapshot_id": self.tf_snapshot_id,
            **{name: self.counters.column(name) for name in PROMOTED_COUNTERS},
            "error_class": None if self.error_class is None else str(self.error_class),
            "error_detail": self.error_detail,
        }

    def row(self) -> dict[str, object]:
        """The `run_stages` row, keyed by the registry's column names."""
        return {
            "run_id": self.run_id,
            "stage": self.stage,
            "seq": self.seq,
            "status": self.status,
            "started_at": _sql_timestamp(self.started_at),
            "ended_at": _sql_timestamp(self.ended_at),
            "snapshot_start": self.snapshot_start,
            "snapshot_end": self.snapshot_end,
            **{name: self.counters.column(name) for name in PROMOTED_COUNTERS},
            "counters": self.counters.payload_json(),
            "error_class": None if self.error_class is None else str(self.error_class),
            "error_detail": self.error_detail,
        }


@dataclass
class RunContext:
    """One CLI invocation: the `runs` row, its stages and its snapshot span (S5.2).

    Used as a context manager, once, around everything the invocation does::

        with RunContext(run_id=..., mode="incremental", ..., source=connect) as run:
            with run.stage("standardize") as stage_run:
                stage_run.finish(exit_code)

    Args:
        run_id: minted or accepted by the CLI; `run-all` mints one and threads it
            through every stage (S4.0).
        mode: a value of :data:`~er.lake.model.RUN_MODES`.
        tenant, config_hash, std_version, survivorship_version: the `runs` row's
            NOT NULL provenance columns, all from the validated document. ``None``
            for an invocation that ran without one, which is then not persisted.
        code_version: :func:`er.versions.code_version` by default — ER-004 already
            owns resolving it, and a second resolver would be a second answer.
        source: where lake connections come from, or ``None`` for telemetry only.
        stream: passed through to the emitter; ``sys.stderr`` by default.
    """

    run_id: str
    mode: str
    tenant: str | None = None
    config_hash: str | None = None
    std_version: str | None = None
    survivorship_version: str | None = None
    code_version: str = field(default_factory=code_version)
    model_version: str | None = None
    tf_snapshot_id: str | None = None
    rebuild_reason: str | None = None
    source: ConnectionSource | None = None
    stream: TextIO | None = None

    started_at: datetime = field(default_factory=_now)
    ended_at: datetime | None = None
    status: str = STATUS_RUNNING
    snapshot_start: int | None = None
    snapshot_end: int | None = None

    def __post_init__(self) -> None:
        if self.mode not in RUN_MODES:
            raise ValueError(f"unknown run mode: {self.mode!r}")
        self._persist = False
        self._failed = False
        self._seq: dict[str, int] = {}
        self._stages: list[StageRun] = []

    @property
    def persisted(self) -> bool:
        """Whether this run is writing rows, as opposed to emitting records only."""
        return self._persist

    @property
    def stages(self) -> tuple[StageRun, ...]:
        """Every stage executed under this run, in execution order."""
        return tuple(self._stages)

    def __enter__(self) -> RunContext:
        self.started_at = _now()
        self._persist = self._begin()
        return self

    def __exit__(self, exc_type: object, exc: BaseException | None, traceback: object) -> None:
        self.ended_at = _now()
        self.status = STATUS_FAILED if self._failed or exc is not None else STATUS_SUCCEEDED
        if not self._persist:
            return
        with self._connect() as connection:
            # Read before the UPDATE: the UPDATE is itself a write and would
            # otherwise be inside the range it is recording.
            self.snapshot_end = current_snapshot(connection)
            _update_row(
                connection,
                _RUNS,
                {
                    "status": self.status,
                    "ended_at": _sql_timestamp(self.ended_at),
                    "snapshot_start": self.snapshot_start,
                    "snapshot_end": self.snapshot_end,
                },
                {"run_id": self.run_id},
            )

    @contextmanager
    def stage(self, name: str, *, declared: Iterable[str] | None = None) -> Iterator[StageRun]:
        """Run one stage: its `run_stages` row, its snapshot range, its S5.2 line.

        The row is written ``running`` on entry and updated with the terminal state
        on exit, whether the body returns or raises. The record is emitted last, so
        it reports what was persisted.

        Args:
            name: the stage name, a value of :data:`~er.lake.model.RUN_STAGES` for a
                pipeline stage. The operator verbs (`doctor`, `assert`, `review`) and
                the `correct` chain wrapper have no value in that closed vocabulary,
                so they emit their record and write no row rather than inventing one.
            declared: the per-stage counter names S4 lists for this stage;
                :data:`~er.obs.counters.DECLARED_COUNTERS` by default.
        """
        vocabulary = DECLARED_COUNTERS.get(name, ()) if declared is None else tuple(declared)
        persist = self._persist and name in RUN_STAGES
        stage_run = StageRun(
            run_id=self.run_id,
            stage=name,
            seq=self._seq.get(name, len(self._seq) + 1),
            started_at=_now(),
            counters=StageCounters(vocabulary),
            config_hash=self.config_hash,
            model_version=self.model_version,
            tf_snapshot_id=self.tf_snapshot_id,
        )
        if persist:
            with self._connect() as connection:
                self._open_stage_row(connection, stage_run)
        self._seq[name] = stage_run.seq
        self._stages.append(stage_run)
        try:
            yield stage_run
        except BaseException as exc:
            # Caught to record the terminal state, re-raised below: a stage that
            # blew up must not also leave a `running` row behind it (S4.7).
            stage_run.finish(
                _exit_code_of(exc),
                error_class=getattr(exc, "error_class", None),
                error_detail=str(exc),
            )
            self._close_stage(stage_run, persist)
            raise
        else:
            if stage_run.exit_code is None:
                # A body that returned without calling `finish` succeeded: there is
                # no exception and no code, and S4.0's success IS the absence of both.
                stage_run.finish(int(ExitCode.SUCCESS))
            self._close_stage(stage_run, persist)

    def _begin(self) -> bool:
        """Write the `runs` row; report whether this run persists anything."""
        provenance = (self.tenant, self.config_hash, self.std_version, self.survivorship_version)
        if self.source is None or any(value is None for value in provenance):
            return False
        try:
            with self.source() as connection:
                if not _relations_present(connection):
                    return False
                if self._run_row_exists(connection):
                    # `runs` is keyed on `run_id` alone (S5.0), and a `run_id` can be
                    # SUPPLIED (`--run-id`, and `--resume` in ER-024). Re-opening it
                    # returns the row to `running` rather than appending a second one;
                    # `started_at` stays as first written, because that is when the run
                    # started.
                    _update_row(
                        connection,
                        _RUNS,
                        {"status": STATUS_RUNNING, "ended_at": None},
                        {"run_id": self.run_id},
                    )
                    return True
                _write_row(
                    connection,
                    _RUNS,
                    {
                        "run_id": self.run_id,
                        "tenant": self.tenant,
                        "mode": self.mode,
                        "status": STATUS_RUNNING,
                        "started_at": _sql_timestamp(self.started_at),
                        "ended_at": None,
                        "config_hash": self.config_hash,
                        "model_version": self.model_version,
                        "tf_snapshot_id": self.tf_snapshot_id,
                        "std_version": self.std_version,
                        "survivorship_version": self.survivorship_version,
                        "code_version": self.code_version,
                        "rebuild_reason": self.rebuild_reason,
                        "snapshot_start": None,
                        "snapshot_end": None,
                    },
                )
        except MissingEnvError:
            # No lake is configured for this process. S5.2's rows are unreachable;
            # its stderr record is not, and it is the half an operator reads.
            return False
        return True

    def _run_row_exists(self, connection: duckdb.DuckDBPyConnection) -> bool:
        row = connection.execute(
            f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{_RUNS} WHERE run_id = ?",
            [self.run_id],
        ).fetchone()
        return row is not None and int(row[0]) > 0

    @contextmanager
    def _connect(self) -> Iterator[duckdb.DuckDBPyConnection]:
        """One connection for one write. Never held across a stage body (S4.0b)."""
        if self.source is None:
            raise RuntimeError("run context has no connection source")
        with self.source() as connection:
            yield connection

    def _open_stage_row(self, connection: duckdb.DuckDBPyConnection, stage_run: StageRun) -> None:
        """Write (or reset) the `running` row, and capture the range's lower bound."""
        existing = self._existing_seq(connection, stage_run.stage)
        if existing is not None:
            stage_run.seq = existing
        else:
            stage_run.seq = self._next_seq(connection)
        stage_run.snapshot_start = current_snapshot(connection)
        if self.snapshot_start is None:
            # S5.2: the run's `snapshot_start` is the version immediately before the
            # FIRST stage begins, not before the `runs` row was written.
            self.snapshot_start = stage_run.snapshot_start
        row = stage_run.row()
        if existing is None:
            _write_row(connection, _RUN_STAGES, row)
            return
        # A re-executed stage updates its row and keeps its `seq` (S5.2): the second
        # row a plain INSERT would leave breaks the `(run_id, stage)` key that makes
        # a stage's snapshot range addressable exactly once per run.
        _update_row(
            connection,
            _RUN_STAGES,
            {name: value for name, value in row.items() if name not in ("run_id", "stage")},
            {"run_id": stage_run.run_id, "stage": stage_run.stage},
        )

    def _existing_seq(self, connection: duckdb.DuckDBPyConnection, stage: str) -> int | None:
        row = connection.execute(
            f"SELECT seq FROM {SCHEMA_QUALIFIER}.{_RUN_STAGES} WHERE run_id = ? AND stage = ?",
            [self.run_id, stage],
        ).fetchone()
        return None if row is None else int(row[0])

    def _next_seq(self, connection: duckdb.DuckDBPyConnection) -> int:
        """The next dense 1-based `seq` for this run (S5.2)."""
        row = connection.execute(
            f"SELECT coalesce(max(seq), 0) FROM {SCHEMA_QUALIFIER}.{_RUN_STAGES} WHERE run_id = ?",
            [self.run_id],
        ).fetchone()
        return (0 if row is None else int(row[0])) + 1

    def _close_stage(self, stage_run: StageRun, persist: bool) -> None:
        """Stamp the terminal state, persist it, then emit the S5.2 record."""
        stage_run.ended_at = _now()
        # Derived by the run, not supplied by the stage: it is the one promoted
        # counter no stage is in a position to measure for itself.
        stage_run.counters.promote("duration_ms", stage_run.duration_ms)
        if stage_run.status == STATUS_FAILED:
            self._failed = True
        if persist:
            with self._connect() as connection:
                stage_run.snapshot_end = current_snapshot(connection)
                row = stage_run.row()
                _update_row(
                    connection,
                    _RUN_STAGES,
                    {name: value for name, value in row.items() if name not in ("run_id", "stage")},
                    {"run_id": stage_run.run_id, "stage": stage_run.stage},
                )
        emit_stage_record(stage_run.record(), stream=self.stream)


def _exit_code_of(exc: BaseException) -> int:
    """The S4.0 status an escaping exception terminates its stage with.

    Deliberately not :func:`er.errors.exit_code_for`: that function is the CLI's
    translation of an exception into a *process* status and returns ``10`` for
    :class:`~er.errors.NothingToDo`, which is a stage that succeeded. Here the
    question is narrower — an exception escaped the stage body — and the answer for
    everything except that one class is "the stage failed".
    """
    code = getattr(exc, "code", None)
    if isinstance(code, bool) or not isinstance(code, int):
        return int(ExitCode.STAGE_FAILURE)
    return code
