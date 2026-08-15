"""The ``er`` command line — the orchestration contract of DesignDoc.md S4.0.

Everything the spec calls "the CLI contract" is here and is testable without a
lake: the command tree, the three global flags, the uniform exit codes, the
``run-all`` chains, and the one stderr JSON line per stage that S5.2 owns.

Four rules govern this module and are stated nowhere else in Python:

* **A stage that has nothing to do is not a stage that failed.** :class:`NoOpStage`
  exits ``10`` and :class:`NotImplementedStage` exits ``1``. The split is what
  stops the S12 M1 exit gate — ``er run-all`` returning ``0`` because every stage
  returned ``10`` — from being satisfied by a command that merely prints. Exit
  ``3`` is reserved for the five named precondition failures of S4.0 and is never
  what an unimplemented stage returns.
* **stdout is the command's output; stderr is telemetry.** ``--json`` switches
  stdout from the human summary to JSONL and never moves or duplicates the stderr
  line (S4 preamble, S5.2).
* **``run-all`` mints ONE ``run_id``** and threads it to every child stage, and it
  never trains — ``er train`` is always a separate invocation (S4.0).
* **Exit codes are derived, never chosen.** A stage raises a classified error from
  ``er.errors`` and :func:`er.errors.exit_code_for` produces the status, so the
  ``error_class`` recorded and the code exited with cannot disagree.

``er init``, ``er lake reset`` (ER-020) and ``er ingest`` (ER-031) are the commands
that do real lake work here; the work itself lives in :mod:`er.lake.init` and
:mod:`er.ingest.landing`, and this module owns only their S4.0 stdout and the exit
status derived from what they returned. Every other command is still a stub.

``er run-all``'s ingest slot is deliberately still a :class:`NoOpStage`: the chain's
composition is ER-014's contract, and wiring the real stage into it belongs with the
ticket that gives ``run-all`` a ``--source``/``--path`` it has to thread.

Every invocation now runs inside an :class:`~er.obs.runctx.RunContext` (ER-023), so
the ``runs`` row, each stage's ``run_stages`` row and its snapshot range are written
by one recorder rather than by each command. Two commands deliberately persist
nothing: `er init` creates the very relations the rows go in, and `er lake reset`
destroys them — and S4.0b forbids holding an attachment across either. They still
emit their stage record, because telemetry is unconditional.

Every mutating command now runs inside the S4.0b writer lock (ER-024), taken before
the :class:`~er.obs.runctx.RunContext` is entered so a refused invocation writes
nothing at all — not even the `runs` row that would otherwise record a run that never
ran (T-CONC-1). ``er run-all --resume`` restarts a failed run from its first
non-`succeeded` stage under that same lock.

``er run-all --mode incremental`` now runs the S4.0 config-drift guard (ER-034) after
the lock is held and before the first stage: the run fingerprint is compared against
the last successful run's, a difference refuses the invocation with exit ``3``, and
``--allow-escalate`` promotes the whole invocation — chain and ``runs.mode`` alike —
to ``--mode full``. The comparison itself is :mod:`er.versions`'.

The dbt subprocess and the ``--vars`` payload moved to ``er.dbt_runner`` with ER-033;
``dbt_vars`` here is that module's :func:`~er.dbt_runner.render_dbt_vars` under its
ER-014 name, not a second builder of the mapping.
"""

from __future__ import annotations

import json
import sys
from collections.abc import Iterator, Mapping, Sequence
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Annotated, Protocol, runtime_checkable

import typer

from er.config.hashing import config_hash
from er.config.loader import ConfigValidationError, load_config
from er.config.schema import Config
from er.dbt_runner import render_dbt_vars
from er.doctor import check_lines, check_record, run_checks
from er.doctor import exit_code as doctor_exit_code
from er.entities.ids import IdFactory, UlidFactory
from er.errors import (
    ConfigError,
    ErError,
    ErrorClass,
    ExitCode,
    PreconditionFailure,
    StageFailure,
    classify,
    exit_code_for,
)
from er.ingest.landing import ingest_delivery
from er.ingest.sources import adapter_for
from er.lake.catalog import tenant_lock
from er.lake.ducklake import connect
from er.lake.env import MissingEnvError
from er.lake.init import init_lake, reset_lake
from er.obs.logging import emit_stage_record
from er.obs.runctx import RunContext, StageRun
from er.resume import ResumePlan, read_resume_rows, resume_plan
from er.versions import (
    ModeDecision,
    RunFingerprint,
    check_mode_preconditions,
    last_successful_run,
)

__all__ = [
    "COMMANDS",
    "MUTATING_COMMANDS",
    "READ_ONLY_COMMANDS",
    "GlobalOptions",
    "NoOpStage",
    "NotImplementedStage",
    "RecordingStage",
    "Stage",
    "app",
    "dbt_vars",
    "emit_stage_line",
    "main",
    "run_all_chain",
]

#: Every command of the S4.0 table, as the path a user types after ``er``.
#: ``lake`` is a group because S4.0 gives ``maintain`` and ``reset`` a row each;
#: ``assert`` and ``review`` are single commands taking a positional sub-verb
#: because S4.0 gives each of them ONE row whose flags vary by verb.
COMMANDS: tuple[str, ...] = (
    "init",
    "doctor",
    "ingest",
    "standardize",
    "train",
    "match",
    "reconcile",
    "assemble",
    "run-all",
    "correct",
    "assert",
    "review",
    "lake maintain",
    "lake reset",
)

#: The stages ``er run-all`` chains. M1 ships them as nothing-to-do stubs so the
#: milestone's exit criterion is runnable before any of them has an implementation
#: (S12 M1); they are exactly the set that may never be a :class:`NotImplementedStage`.
_CHAINED_STAGES: frozenset[str] = frozenset(
    {"ingest", "standardize", "match", "reconcile", "assemble"}
)

#: The two values ``--mode`` accepts on ``er run-all`` and ``er match`` (S4.0).
_MODES: tuple[str, str] = ("incremental", "full")

#: ``runs.mode`` for a standalone stage invocation outside a ``run-all`` chain (S5);
#: the chains and the lifecycle verbs pass their own value.
_MODE_STAGE: str = "stage"

#: Every command that mutates the namespace and therefore takes the S4.0b writer
#: lock, as the path a user types after ``er``. S4.7 names this set; ``assert`` is
#: here for all three of its verbs, and ``review`` only for ``resolve``.
MUTATING_COMMANDS: frozenset[str] = frozenset(
    {
        "init",
        "ingest",
        "standardize",
        "train",
        "match",
        "reconcile",
        "assemble",
        "run-all",
        "correct",
        "assert",
        "review resolve",
        "lake maintain",
        "lake reset",
    }
)

#: The two commands S4.7 excludes from the lock: both only read. Stated as its own
#: set rather than inferred from the complement, so a command added to S4.0's table
#: and to neither set here is a visible omission rather than a silent read-only one.
READ_ONLY_COMMANDS: frozenset[str] = frozenset({"doctor", "review list"})

#: The mutating command that takes the lock deeper down instead of here.
#: :func:`~er.lake.init.reset_lake` drops the catalog metadata schema on the lock's
#: OWN connection, because the drop has to happen inside the critical section rather
#: than merely after it — so taking the lock here as well would be a second session
#: contending with the first, and every `er lake reset` would refuse itself.
_LOCKED_DEEPER: frozenset[str] = frozenset({"lake reset"})


@dataclass(frozen=True)
class GlobalOptions:
    """The three flags every S4.0 command accepts, after resolution.

    Resolution is deliberately eager: the config is read and validated before any
    stage runs and before any lake environment variable is read, which is what
    makes "an invalid document exits 2 before any connection is opened" (S4.0) a
    property of the control flow rather than a convention each command follows.
    """

    #: The document that was loaded, or ``None`` for a command whose S4.0 required
    #: env does not include ``ER_CONFIG`` and which was invoked without one.
    config: Config | None
    #: SHA-256 over the canonicalised validated document (S5.2); ``None`` with
    #: :attr:`config`.
    config_hash: str | None
    #: Supplied with ``--run-id`` or minted here; ``run-all`` mints exactly once.
    run_id: str
    #: ``--json``: stdout becomes JSONL. It never affects stderr.
    json_output: bool

    @classmethod
    def resolve(
        cls,
        *,
        config_path: Path | None,
        run_id: str | None,
        json_output: bool,
        require_config: bool = True,
        id_factory: IdFactory | None = None,
    ) -> GlobalOptions:
        """Build the options a command runs under.

        Args:
            config_path: ``--config``, already defaulted from ``$ER_CONFIG`` by the
                option's ``envvar``.
            run_id: ``--run-id``; minted when absent.
            json_output: ``--json``.
            require_config: whether ``ER_CONFIG`` is in this command's S4.0
                required-env column. ``er init``, ``er doctor`` and
                ``er lake maintain`` list lake env only, so they run without a
                document — but they still validate one that is present, because a
                document that cannot be read is a fault wherever it is noticed.
            id_factory: the source of the minted id; production ULIDs by default.

        Raises:
            typer.Exit: exit ``2`` when the document fails S6 validation. The
                offending JSON pointer is printed to stderr first (S4.0, S6.1).
        """
        document: Config | None = None
        if config_path is not None or require_config:
            document = _load_config(config_path)
        factory = id_factory if id_factory is not None else UlidFactory()
        return cls(
            config=document,
            config_hash=None if document is None else config_hash(document),
            run_id=run_id if run_id else factory.new(),
            json_output=json_output,
        )


def _load_config(config_path: Path | None) -> Config:
    """Load and validate the S6 document, or exit ``2`` naming the pointer."""
    try:
        return load_config(config_path)
    except ConfigValidationError as exc:
        # S4.0 requires the offending JSON pointer on the way out; S6.1's message
        # key travels with it because it is the token an operator greps the spec
        # for. stderr, not stdout: stdout stays parseable by the caller.
        sys.stderr.write(f"config error at {exc.pointer}: {exc}\n")
        raise typer.Exit(exit_code_for(exc)) from exc


class Stage(Protocol):
    """One unit of pipeline work, addressed by the name S4/S5 records for it.

    Read-only properties rather than attributes so a frozen implementation, which
    every implementation here is, satisfies the protocol.
    """

    @property
    def name(self) -> str:
        """The stage name written to ``run_stages.stage`` and the log line."""

    @property
    def args(self) -> tuple[str, ...]:
        """The flags this stage runs under, as the user would have typed them."""

    def run(self, options: GlobalOptions) -> int:
        """Do the work and return an S4.0 exit code, or raise an ``ErError``."""


@runtime_checkable
class RecordingStage(Protocol):
    """A stage that reports counters, and so needs the row it is being recorded in.

    S5.2 puts a stage's counters on its own ``run_stages`` row, and
    :class:`~er.obs.runctx.StageRun` is that row — but it is minted by
    :meth:`~er.obs.runctx.RunContext.stage` *around* the body, so a stage cannot be
    constructed holding one. :meth:`bind` is how :func:`_execute` hands it over,
    and it is opt-in rather than a parameter of :meth:`Stage.run` because the M1
    stubs have nothing to count and a fourth argument they all ignored would be a
    signature every future stage had to satisfy to say nothing.
    """

    def bind(self, stage_run: StageRun) -> None:
        """Receive the ``run_stages`` row this execution records into."""


@dataclass(frozen=True)
class NoOpStage:
    """A stage with nothing to do: exit ``10`` (S4.0, S12 M1).

    It is a real stage — it is executed, and it emits its own S5.2 line — that
    happens to find no work. That is what distinguishes it from
    :class:`NotImplementedStage`, and it is why a chain of these is a *successful*
    run rather than a run that skipped its work.
    """

    name: str
    args: tuple[str, ...] = ()

    def run(self, options: GlobalOptions) -> int:
        return int(ExitCode.NOTHING_TO_DO)


@dataclass(frozen=True)
class NotImplementedStage:
    """A stage that does not exist yet: exit ``1`` (S4.0).

    Never ``10`` and never ``0``. A ``10`` would make an unwritten stage
    indistinguishable from one that ran and found no work, and would let the S12
    M1 exit gate pass on a pipeline that does nothing at all.
    """

    name: str
    args: tuple[str, ...] = ()

    def run(self, options: GlobalOptions) -> int:
        raise StageFailure(f"stage not implemented: {self.name}")


@dataclass(frozen=True)
class _InitStage:
    """`er init`: apply the `ddl.py`-owned registry to the namespace (S4.0, S5.1).

    Writes its own stdout — S4.0 gives the command "one line per relation" — and
    nothing else: the ``--json`` stream is a sequence of ``{relation, action}``
    objects with no summary object among them.
    """

    args: tuple[str, ...] = ()
    name: str = "init"

    def run(self, options: GlobalOptions) -> int:
        for result in init_lake(tenant=_tenant_of(options)):
            _write_stdout(
                {"relation": result.relation, "action": result.action.value},
                f"{result.relation} {result.action.value}",
                options,
            )
        return int(ExitCode.SUCCESS)


@dataclass(frozen=True)
class _ResetStage:
    """`er lake reset`: destroy the namespace behind ``--confirm-tenant`` (S4.0).

    The guard is not applied here. :func:`~er.lake.init.reset_lake` compares the
    flag against the validated config before it opens anything, so there is one
    place a typo can be caught and it is the one that also takes the lock.
    """

    confirm_tenant: str
    args: tuple[str, ...] = ()
    name: str = "reset"

    def run(self, options: GlobalOptions) -> int:
        result = reset_lake(tenant=_tenant_of(options), confirm_tenant=self.confirm_tenant)
        _write_stdout(
            {"dropped_schema": result.dropped_schema, "deleted_prefix": result.deleted_prefix},
            f"dropped_schema={result.dropped_schema}, deleted_prefix={result.deleted_prefix}",
            options,
        )
        return int(ExitCode.SUCCESS)


@dataclass
class _IngestStage:
    """`er ingest`: append a delivery to `raw_records` as version history (S4.1).

    Mutable, unlike the other stages here, for exactly one reason: it is the first
    stage with counters to report, so it holds the :class:`~er.obs.runctx.StageRun`
    :meth:`bind` hands it. Everything else it does is
    :func:`~er.ingest.landing.ingest_delivery`'s — this class owns the S4.0 surface
    (the manifest line, and the exit status derived from the manifest) and not the
    write.

    ``--full-refresh-keys`` is passed through and never interpreted here. S4.1.1's
    tombstone derivation, its empty-delivery guard and resurrection counting all need
    the delivery and the live key set in front of them, so they live in
    :func:`~er.ingest.landing.ingest_delivery` with the rest of the write; the guard's
    exit `2` reaches the process the way every other classified failure does, through
    :func:`~er.errors.exit_code_for`.
    """

    source: str
    path: Path
    full_refresh_keys: bool = False
    args: tuple[str, ...] = ()
    name: str = "ingest"
    stage_run: StageRun | None = None

    def bind(self, stage_run: StageRun) -> None:
        self.stage_run = stage_run

    def run(self, options: GlobalOptions) -> int:
        if options.config is None or self.stage_run is None:
            # Unreachable through the command tree: S4.0 lists `ER_CONFIG` in this
            # command's required-env column, so `GlobalOptions.resolve` has already
            # exited 2 without a document, and `_execute` binds before the body runs.
            # Stated rather than asserted, so a stage driven some other way fails
            # loudly instead of ingesting against a half-built invocation.
            raise StageFailure("er ingest was invoked without a config or a run_stages row")
        with connect() as connection:
            manifest = ingest_delivery(
                options.config,
                self.source,
                self.path,
                connection,
                self.stage_run,
                full_refresh_keys=self.full_refresh_keys,
            )
        _write_stdout(manifest.manifest(), manifest.stdout_line(), options)
        return manifest.exit_code


def _tenant_of(options: GlobalOptions) -> str | None:
    """The tenant this invocation runs under, or ``None`` when it has no document.

    S4.0's required-env column for `er init` names lake variables only, so a run
    without an S6 document is legitimate and the tenant is then simply unknown.
    """
    return None if options.config is None else options.config.tenant


def _require_known_source(options: GlobalOptions, source: str, path: Path) -> None:
    """Refuse an unknown ``--source`` before any lake connection is opened (S4.0).

    S4.0 puts "unknown source" in the same exit-`2` row as a failed Pydantic
    validation, and gives that row the same timing: *before any connection is
    opened*. The stage body cannot supply that, because the writer lock and the
    `runs` row are both taken before it runs — so the check happens here, where
    ``--mode`` and `run-all`'s ``--source``/``--path`` pairing are also checked.

    :func:`~er.ingest.sources.adapter_for` is THE judge of whether a source is
    known, and it is pure: it resolves `sources.<name>` and its adapter token and
    touches no file. Calling it again inside the stage is the same question asked of
    the same function, not a second vocabulary of source names.

    Raises:
        typer.Exit: exit ``2``, after naming the offending config key on stderr.
    """
    if options.config is None:
        return
    try:
        adapter_for(options.config, source, path)
    except ErError as exc:
        # stderr, not stdout: a caller piping the manifest must not have to parse a
        # refusal out of it.
        sys.stderr.write(f"{exc}\n")
        raise typer.Exit(exit_code_for(exc)) from exc


@contextmanager
def _writer_lock(command: str, options: GlobalOptions) -> Iterator[None]:
    """Hold the S4.0b writer lock for ``command``, or refuse the invocation (S4.7).

    Entered **outside** the :class:`~er.obs.runctx.RunContext` by every caller, which
    is the whole point: S4.7 requires a refused writer to write nothing at all, and a
    `runs` row minted before the lock was tried would be a ledger entry for a run that
    never ran. T-CONC-1 asserts exactly that absence.

    Two invocations pass through unlocked, and both are the same judgement the rest of
    this module already makes:

    * a command with no validated document has no ``tenant`` to key the lock on — S4.0
      lists lake env only for `er init`, `er doctor` and `er lake maintain`, so running
      without one is legitimate — exactly as :func:`_run_context` persists nothing when
      the document is absent;
    * a process with no ``$ER_CATALOG_DSN`` has no catalog to take a lock on, and so no
      second writer that could exist to contend with it. This is what lets the S8.4
      unit layer exercise the command tree on a bare runner with no services.

    Raises:
        typer.Exit: exit ``3`` when another writer holds the lock, after writing S4.7's
            literal message to stderr.
    """
    tenant = _tenant_of(options)
    if tenant is None or command not in MUTATING_COMMANDS or command in _LOCKED_DEEPER:
        yield
        return
    with ExitStack() as held:
        try:
            held.enter_context(tenant_lock(tenant, run_id=options.run_id))
        except MissingEnvError:
            pass
        except PreconditionFailure as exc:
            # stderr, not stdout: stdout is the command's output, and a caller piping
            # it must not have to parse a refusal out of the manifest it asked for.
            sys.stderr.write(f"{exc}\n")
            raise typer.Exit(exit_code_for(exc)) from exc
        yield


def _stage_for(name: str, args: Sequence[str] = ()) -> Stage:
    """The stage M1 ships for ``name``: a no-op stub if ``run-all`` chains it."""
    flags = tuple(args)
    if name in _CHAINED_STAGES:
        return NoOpStage(name=name, args=flags)
    return NotImplementedStage(name=name, args=flags)


def run_all_chain(
    mode: str,
    skip_ingest: bool,
    *,
    source: str | None = None,
    path: str | None = None,
) -> list[Stage]:
    """The ordered stages ``er run-all --mode <mode>`` executes (S4.0).

    ``train`` is absent from both chains and that is normative: ``er run-all``
    NEVER trains, because a model version that changes underneath a chain would
    make the run's scores unreadable against any earlier one.

    Args:
        mode: ``incremental`` or ``full``.
        skip_ingest: drop the leading ``ingest`` stage.
        source: ``--source`` for the ingest stage, when it runs.
        path: ``--path`` for the ingest stage, when it runs.

    Raises:
        ValueError: ``mode`` is neither ``incremental`` nor ``full``.
    """
    if mode not in _MODES:
        raise ValueError(f"unknown run-all mode: {mode!r}")
    incremental = mode == "incremental"
    stages: list[Stage] = []
    if not skip_ingest:
        ingest_args: list[str] = []
        if source is not None:
            ingest_args += ["--source", source]
        if path is not None:
            ingest_args += ["--path", path]
        stages.append(_stage_for("ingest", ingest_args))
    stages.append(_stage_for("standardize", ("--changed-only",) if incremental else ()))
    stages.append(_stage_for("match", ("--mode", mode)))
    stages.append(_stage_for("reconcile"))
    stages.append(_stage_for("assemble", ("--touched-only",) if incremental else ()))
    return stages


#: THE emitter of the S5.2 stderr line, under its ER-014 name.
#:
#: Bound rather than wrapped, for the reason ER-014 gave it: "exactly one line per
#: stage" is only checkable while exactly one function writes one. ER-023 moved the
#: body to :mod:`er.obs.logging`, next to :data:`~er.obs.logging.STAGE_RECORD_KEYS`,
#: which is what it now validates the record against.
#:
#: ``--json`` is not a parameter of it: that flag switches *stdout*, and moving this
#: line would break the guarantee that a caller can pipe stdout and still read
#: telemetry.
emit_stage_line = emit_stage_record


#: The ``--vars`` payload every dbt invocation carries (S6, S4.2, S4.6).
#:
#: THE builder, and it lives in ``er.dbt_runner`` next to the subprocess that
#: passes it (ER-033). Bound here rather than wrapped: a wrapper would be a second
#: function able to produce a payload, and "``dbt_project.yml`` holds only
#: fallbacks" is only true while exactly one function renders the override.
dbt_vars = render_dbt_vars


@dataclass
class _Outcome:
    """What one stage did, in the terms both output streams need."""

    stage: str
    exit_code: int
    status: str
    error_class: ErrorClass | None = None
    error_detail: str | None = None
    record: dict[str, object] = field(default_factory=dict)


def _run_context(
    options: GlobalOptions,
    *,
    mode: str,
    persist: bool = True,
    model_version: str | None = None,
) -> RunContext:
    """The recorder this invocation runs inside (S5.2).

    ``persist=False`` is for the two namespace lifecycle verbs and nothing else:
    `er init` creates the relations the rows would go in, and `er lake reset` drops
    them, so both would be writing to a target that does not exist on one side of
    the command. Their S5.2 stderr record is emitted either way.

    An invocation with no validated document also persists nothing — `runs` requires
    ``tenant``, ``config_hash``, ``std_version`` and ``survivorship_version``, and
    all four come from the document — which :class:`~er.obs.runctx.RunContext`
    decides for itself from the values it is given.

    ``model_version`` is supplied only by `--resume`, which pins the resumed run to
    the one it was recorded under (S4.7). Every other invocation leaves it ``None``
    and the stage that resolves the ``status='active'`` row sets it.
    """
    document = options.config
    return RunContext(
        run_id=options.run_id,
        mode=mode,
        tenant=None if document is None else document.tenant,
        config_hash=options.config_hash,
        model_version=model_version,
        std_version=None if document is None else document.versions.std_version,
        survivorship_version=(None if document is None else document.versions.survivorship_version),
        source=connect if persist else None,
    )


def _execute(stage: Stage, options: GlobalOptions, run: RunContext) -> _Outcome:
    """Run one stage inside its `run_stages` row, whatever the stage did.

    The row, its snapshot range and the S5.2 line are all the context's; this
    function's only job is to turn a raised :class:`~er.errors.ErError` into the
    exit code S4.0 derives from its class, and to hand both to
    :meth:`~er.obs.runctx.StageRun.finish` so the class recorded and the code
    exited with cannot disagree.

    A :class:`RecordingStage` is handed its row before the body runs, so the counters
    it writes are on the row the context persists rather than on one it built for
    itself (S5.2).
    """
    with run.stage(stage.name) as stage_run:
        if isinstance(stage, RecordingStage):
            stage.bind(stage_run)
        try:
            code = stage.run(options)
        except ErError as exc:
            code = exit_code_for(exc)
            # Classified through the S4.7 table rather than off the exception, so the
            # value reaching `run_stages.error_class` is always one of the seven rows.
            stage_run.finish(code, error_class=classify(exc), error_detail=exc.detail)
        else:
            stage_run.finish(code)
    return _Outcome(
        stage=stage.name,
        exit_code=code,
        status=stage_run.status,
        error_class=stage_run.error_class,
        error_detail=stage_run.error_detail,
        record=stage_run.record(),
    )


def _outcome_phrase(code: int) -> str:
    """The human rendering of an S4.0 exit code, for the non-``--json`` stdout."""
    if code == ExitCode.SUCCESS:
        return "ok"
    if code == ExitCode.NOTHING_TO_DO:
        return "nothing to do"
    return "failed"


def _write_stdout(payload: Mapping[str, object], human: str, options: GlobalOptions) -> None:
    """One stdout line: JSON under ``--json``, the human summary otherwise."""
    if options.json_output:
        line = json.dumps(dict(payload), separators=(",", ":"), ensure_ascii=False)
        sys.stdout.write(line + "\n")
    else:
        sys.stdout.write(human + "\n")


def _report(outcome: _Outcome, options: GlobalOptions) -> None:
    """The per-stage stdout line S4.0's "stdout" column calls for."""
    detail = "" if outcome.error_detail is None else f": {outcome.error_detail}"
    _write_stdout(
        {
            "stage": outcome.stage,
            "status": outcome.status,
            "exit_code": outcome.exit_code,
            "error_detail": outcome.error_detail,
        },
        f"{outcome.stage}: {_outcome_phrase(outcome.exit_code)}{detail}",
        options,
    )


def _run_single(
    name: str,
    options: GlobalOptions,
    args: Sequence[str] = (),
    *,
    mode: str = _MODE_STAGE,
    command: str | None = None,
) -> None:
    """Execute one stage as a standalone command and exit with its code.

    ``typer.Exit`` is raised after the context closes, never inside it: an exception
    escaping the stage body is how :class:`~er.obs.runctx.RunContext` learns the run
    failed, and an ordinary exit code is not a failure.

    Args:
        command: the S4.0 command path, when it differs from the stage name — `er lake
            maintain` writes a `maintain` stage, and `er review` is two commands under
            one stage. It decides whether the writer lock is taken (S4.7).
    """
    with (
        _writer_lock(name if command is None else command, options),
        _run_context(options, mode=mode) as run,
    ):
        outcome = _execute(_stage_for(name, args), options, run)
    _report(outcome, options)
    raise typer.Exit(outcome.exit_code)


def _run_command(
    stage: Stage, options: GlobalOptions, *, mode: str, command: str, persist: bool = False
) -> None:
    """Execute a stage that writes its OWN stdout, and exit with its code.

    Distinct from :func:`_run_single` in exactly one way, and it is a contract and
    not a preference: S4.0 gives `er init`, `er lake reset` and `er ingest` a stdout
    of their own — one line per relation, the reset summary, the S4.1 manifest line —
    so the per-stage summary :func:`_report` writes would be a second, differently
    shaped line in a ``--json`` stream that S4.0 says holds only the command's output.

    The failure message is repeated as a bare stderr line beside the S5.2 record.
    The record is telemetry a caller parses; the line is what an operator reads, and
    for `er init` it is the literal S4.0 immutability message.

    ``persist`` is false for the two namespace lifecycle verbs and true for every
    real stage: `er init` creates the relations the rows go in and `er lake reset`
    drops them, while `er ingest` is an ordinary writer whose `runs` and `run_stages`
    rows are exactly what S5.2 asks for (see :func:`_run_context`).
    """
    with (
        _writer_lock(command, options),
        _run_context(options, mode=mode, persist=persist) as run,
    ):
        outcome = _execute(stage, options, run)
    if outcome.error_detail is not None:
        sys.stderr.write(outcome.error_detail + "\n")
    raise typer.Exit(outcome.exit_code)


def _run_chain(
    stages: Sequence[Stage],
    options: GlobalOptions,
    *,
    mode: str,
    model_version: str | None = None,
) -> None:
    """Execute a chain, apply the S4.0 propagation rule, and exit with its code.

    ``10`` never aborts: a stage with nothing to do leaves the downstream stages
    to discover the same thing for themselves. The first exit that is neither
    ``0`` nor ``10`` stops the chain and becomes the process's status.

    Unlike :func:`_run_single`, this does NOT take the writer lock: its one caller
    holds it already, because with ``--resume`` the chain itself is only knowable from
    `run_stages`, and reading that ledger while another writer is midway through it
    would plan a restart from a stage that is running.
    """
    final = int(ExitCode.SUCCESS)
    executed = 0
    with _run_context(options, mode=mode, model_version=model_version) as run:
        for stage in stages:
            outcome = _execute(stage, options, run)
            executed += 1
            _report(outcome, options)
            if outcome.exit_code not in (ExitCode.SUCCESS, ExitCode.NOTHING_TO_DO):
                final = outcome.exit_code
                break
    _write_stdout(
        {"run_id": options.run_id, "stages": executed, "exit_code": final},
        f"run {options.run_id}: {executed} stage(s), exit {final}",
        options,
    )
    raise typer.Exit(final)


def _resume(run_id: str, options: GlobalOptions) -> ResumePlan:
    """The S4.7 plan for ``--resume <run_id>``, or the refusal it earns.

    Called with the writer lock already held, so the ledger it reads cannot change
    underneath it. The refusals are :mod:`er.resume`'s — exit ``2`` on a config-hash
    mismatch, exit ``3`` on a run with nothing to resume — and this only gives them
    the run id and a stderr line, because an operator who typed a `run_id` needs to
    see which one was refused.
    """
    if options.config_hash is None:
        # Unreachable through the command tree: `run-all` lists ER_CONFIG in its S4.0
        # required-env column, so `GlobalOptions.resolve` has already exited 2 without
        # a document. Stated rather than asserted, so the value that is compared
        # against `runs.config_hash` can never be a `None` that matches nothing.
        raise typer.Exit(
            _refuse(run_id, ConfigError("--resume needs the S6 document the run was hashed from"))
        )
    try:
        with connect() as connection:
            rows = read_resume_rows(connection, run_id)
        return resume_plan(rows, options.config_hash)
    except ErError as exc:
        raise typer.Exit(_refuse(run_id, exc)) from exc


def _guarded_mode(mode: str, options: GlobalOptions, *, allow_escalate: bool) -> str:
    """The S4.0 config-drift guard: the mode to run under, or the refusal it earns.

    Called with the writer lock already held and before the first stage runs, which is
    what makes a refusal an S4.7 `precondition` rather than a failed run: no `runs`
    row, no `run_stages` row and no committed snapshot exist yet to be left behind.

    `--mode full` and a namespace with no prior successful run both reach
    :func:`~er.versions.check_mode_preconditions` and are decided there, not short-cut
    here — one guard, one place it says yes.

    Raises:
        typer.Exit: exit `3` when an incremental run's fingerprint has drifted and
            ``--allow-escalate`` was not given, after naming every drifted field and
            both its values on stderr.
    """
    document = options.config
    if document is None or options.config_hash is None:
        # Unreachable through the command tree: S4.0 lists ER_CONFIG in `run-all`'s
        # required-env column, so `GlobalOptions.resolve` has already exited 2 without
        # a document. Stated rather than asserted, so a guard with nothing to compare
        # cannot silently compare two `None`s and call them equal.
        return mode
    current = RunFingerprint(
        config_hash=options.config_hash,
        # The value this run's `runs.model_version` will hold, which is NULL until a
        # stage resolves the `status='active'` row — the same value `_run_chain` hands
        # `_run_context` on this path. The two MUST move together: a prior run recorded
        # under a resolved `model_version` would otherwise drift against a current one
        # that has merely not been resolved yet.
        model_version=None,
        std_version=document.versions.std_version,
        survivorship_version=document.versions.survivorship_version,
    )
    decision = check_mode_preconditions(
        _last_successful_run(document.tenant), current, mode, allow_escalate=allow_escalate
    )
    return _apply_mode_decision(decision)


def _last_successful_run(tenant: str) -> RunFingerprint | None:
    """The baseline the guard compares against, or ``None`` when there is no lake.

    A process with no lake environment has no `runs` relation to read — that is how
    the S8.4 unit layer drives the command tree on a bare runner — and a first run
    cannot drift. Both are the same answer, exactly as
    :class:`~er.obs.runctx.RunContext` treats a missing environment as "persist
    nothing" rather than as a failure of the run.
    """
    try:
        with connect() as connection:
            return last_successful_run(connection, tenant)
    except MissingEnvError:
        return None


def _apply_mode_decision(decision: ModeDecision) -> str:
    """Report the guard's verdict and return the mode the chain is built from.

    stderr for both lines, not stdout: S4.0 gives `run-all`'s stdout one line per
    stage plus a run summary, and a caller piping it must not have to parse a refusal
    out of it.
    """
    if decision.refused:
        # Raised as a classified failure rather than exiting `3` directly, so the
        # status comes from the S4.7 table the same way every other refusal's does.
        refusal = PreconditionFailure(decision.message)
        sys.stderr.write(f"{refusal}\n")
        raise typer.Exit(exit_code_for(refusal))
    if decision.escalated:
        sys.stderr.write(f"{decision.message}\n")
    return decision.mode


def _refuse(run_id: str, exc: ErError) -> int:
    """Report a `--resume` refusal on stderr and return its S4.0 exit status."""
    sys.stderr.write(f"--resume {run_id}: {exc}\n")
    return exit_code_for(exc)


def _resume_chain(plan: ResumePlan, stages: Sequence[Stage]) -> list[Stage]:
    """``stages`` from :attr:`~er.resume.ResumePlan.resume_from` onwards (S4.7).

    Sliced, never filtered: S4.7 restarts *the chain* from the failed stage, so every
    stage after it runs again too — the failed stage's downstream neighbours never ran
    at all, and the ones that did are before the slice.

    Raises:
        er.errors.PreconditionFailure: the stage to restart from is not in this chain,
            which is what `--resume` of a run that ingested plus ``--skip-ingest``
            looks like. Exit ``3``, and no stage runs.
    """
    names = [stage.name for stage in stages]
    if plan.resume_from not in names:
        raise PreconditionFailure(
            f"run {plan.run_id} stopped at stage {plan.resume_from!r}, "
            f"which is not in the {plan.mode} chain being resumed: {names}"
        )
    return list(stages[names.index(plan.resume_from) :])


# --- the S4.0 command tree -------------------------------------------------
#
# The three global options are repeated on every command rather than declared on
# a group callback: S4.0 says they are "accepted by every command", and a click
# group parameter is only accepted BEFORE the subcommand name, so `er ingest
# --config x` would be rejected if they lived on the callback.

ConfigOption = Annotated[
    Path | None,
    typer.Option("--config", envvar="ER_CONFIG", help="S6 config document (default: $ER_CONFIG)."),
]
RunIdOption = Annotated[
    str | None,
    typer.Option("--run-id", help="ULID for this run (default: minted)."),
]
JsonOption = Annotated[
    bool,
    typer.Option("--json", help="Emit JSONL on stdout instead of the human summary."),
]

app = typer.Typer(
    name="er",
    help="Entity resolution and golden record pipeline (DesignDoc.md S4.0).",
    no_args_is_help=True,
    add_completion=False,
)
lake_app = typer.Typer(help="Namespace maintenance and destruction (S4.0, S4.0b).")
app.add_typer(lake_app, name="lake")


def _mode_option(mode: str) -> str:
    """Validate ``--mode``; a bad value is a usage error, which click exits ``2``."""
    if mode not in _MODES:
        raise typer.BadParameter(f"must be one of {', '.join(_MODES)}")
    return mode


@app.command()
def init(
    force: Annotated[
        bool, typer.Option("--force", help="Recorded on the stage; recreates nothing.")
    ] = False,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Create the ddl.py-owned relations in the attached lake (S4.0, S5.0).

    ``--force`` is accepted because S4.0's flag column lists it, and it recreates
    nothing: S5.0 puts every statement against a `ddl.py`-owned relation in
    :mod:`er.lake.ddl`, so a destructive recreate would have to emit a ``DROP`` from
    outside the module that owns the ownership guard. It travels on the stage record
    so an operator can see what was asked for.
    """
    options = GlobalOptions.resolve(
        config_path=config, run_id=run_id, json_output=json_output, require_config=False
    )
    _run_command(
        _InitStage(args=("--force",) if force else ()), options, mode="init", command="init"
    )


@app.command()
def doctor(
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Assert every pinned version and runtime invariant (S2.1, T-DOCTOR-1).

    The one command that is neither a :class:`Stage` nor run inside a
    :class:`~er.obs.runctx.RunContext`, and both halves of that are S5's decision
    rather than a shortcut: ``run_stages.stage`` has no ``doctor`` value in the S5
    enum, so there is no row this could legally write, and the doctor mutates nothing
    — it is already in :data:`READ_ONLY_COMMANDS`, so it takes no writer lock and a
    second concurrent `er doctor` is not a second writer.

    Exit ``1`` on any failed check, never ``3``: a pin mismatch is a check failure
    under S4.0's exit-code table (S2.1 says so in as many words). Every check prints
    first, including the ones after a failure.
    """
    options = GlobalOptions.resolve(
        config_path=config, run_id=run_id, json_output=json_output, require_config=False
    )
    results = run_checks()
    for result, line in zip(results, check_lines(results), strict=True):
        _write_stdout(check_record(result), line, options)
    raise typer.Exit(doctor_exit_code(results))


@app.command()
def ingest(
    source: Annotated[str, typer.Option("--source", help="Source system name from S6.")],
    path: Annotated[Path, typer.Option("--path", help="Delivery directory.")],
    full_refresh_keys: Annotated[
        bool,
        typer.Option("--full-refresh-keys", help="Treat the delivery as the complete key set."),
    ] = False,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Append source deliveries to raw_records as version history (S4.1).

    ``--path`` is the drop-folder ROOT, not the source directory: S4.1 reads
    ``storage.drop_dir/<source>/``, and this flag overrides the root half of that
    (:func:`~er.ingest.sources.discover_files`).
    """
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    args = ["--source", source, "--path", str(path)]
    if full_refresh_keys:
        args.append("--full-refresh-keys")
    _require_known_source(options, source, path)
    _run_command(
        _IngestStage(
            source=source, path=path, full_refresh_keys=full_refresh_keys, args=tuple(args)
        ),
        options,
        mode=_MODE_STAGE,
        command="ingest",
        persist=True,
    )


@app.command()
def standardize(
    changed_only: Annotated[
        bool, typer.Option("--changed-only", help="Process unprocessed batches only.")
    ] = False,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Run the dbt staging and intermediate models (S4.2)."""
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    _run_single("standardize", options, ("--changed-only",) if changed_only else ())


@app.command()
def train(
    if_changed: Annotated[
        bool,
        typer.Option("--if-changed", help="Exit 10 when (config_hash, corpus_snapshot) is known."),
    ] = False,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Fit a Splink model and register a new model_version (S4.3.2).

    Never reached from ``er run-all`` or ``er correct``: training is always an
    explicit, separate invocation (S4.0).
    """
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    _run_single("train", options, ("--if-changed",) if if_changed else (), mode="train")


@app.command()
def match(
    mode: Annotated[str, typer.Option("--mode", callback=_mode_option, help="incremental | full.")],
    model_version: Annotated[
        str | None, typer.Option("--model-version", help="Default: the status='active' row.")
    ] = None,
    new_tf_snapshot: Annotated[
        bool,
        typer.Option("--new-tf-snapshot", help="Rebuild tf_lookup; --mode full, er correct only."),
    ] = False,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Score candidate pairs into match_scores (S4.3)."""
    if new_tf_snapshot and mode != "full":
        # D4: outside `er train`, `er correct` is the ONLY path that mints a
        # tf_snapshot_id, and it does so through `--mode full`.
        raise typer.BadParameter("--new-tf-snapshot requires --mode full", param_hint="--mode")
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    args = ["--mode", mode]
    if model_version is not None:
        args += ["--model-version", model_version]
    if new_tf_snapshot:
        args.append("--new-tf-snapshot")
    _run_single("match", options, args)


@app.command()
def reconcile(
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Cluster the assertion-adjusted edge set and reconcile entities (S4.5)."""
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    _run_single("reconcile", options)


@app.command()
def assemble(
    touched_only: Annotated[
        bool, typer.Option("--touched-only", help="Rebuild only this run's touched set.")
    ] = False,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Build golden records, lineage and display rows (S4.6)."""
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    _run_single("assemble", options, ("--touched-only",) if touched_only else ())


@app.command("run-all")
def run_all(
    mode: Annotated[str, typer.Option("--mode", callback=_mode_option, help="incremental | full.")],
    source: Annotated[
        str | None, typer.Option("--source", help="Source system for ingest.")
    ] = None,
    path: Annotated[Path | None, typer.Option("--path", help="Delivery directory.")] = None,
    skip_ingest: Annotated[
        bool, typer.Option("--skip-ingest", help="Start the chain at standardize.")
    ] = False,
    allow_escalate: Annotated[
        bool, typer.Option("--allow-escalate", help="Promote to --mode full on config drift.")
    ] = False,
    resume: Annotated[
        str | None, typer.Option("--resume", help="Restart RUN_ID at its first unfinished stage.")
    ] = None,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Run the whole chain under one run_id (S4.0).

    It never trains, and it mints exactly one ``run_id`` for every child stage.

    ``--mode incremental`` is guarded: the run fingerprint `(config_hash,
    model_version, std_version, survivorship_version)` is compared against the last
    successful run for this tenant, and a difference refuses the invocation with exit
    ``3`` before any stage runs. ``--allow-escalate`` promotes the run to ``--mode
    full`` instead — the chain S5.1 requires after a version bump. ``--mode full`` is
    never guarded: it rebuilds exactly what the drift invalidated.

    ``--resume RUN_ID`` restarts that run from its first non-`succeeded` stage under
    its original `run_id` and `config_hash` (S4.7). The chain it restarts is the one
    the run was recorded under — `runs.mode`, not the ``--mode`` on this command line:
    the stages already in `run_stages` belong to that chain, and re-executing the rest
    of a different one would append stages to a run they were never part of. ``--mode``
    stays required because S4.0's table requires it. A resumed run is NOT re-guarded:
    S4.7 gives `--resume` its own config-hash precondition with its own exit code
    (``2``), and running both against one invocation would refuse the same drift twice
    under two different codes.
    """
    if not skip_ingest and (source is None or path is None):
        # Rejected before ANY stage runs, and exit 2 rather than 3: this is a
        # malformed invocation, not an unmet precondition of the lake (S4.0).
        raise typer.BadParameter(
            "--source and --path are required unless --skip-ingest is set",
            param_hint="--source/--path",
        )
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    delivery = None if path is None else str(path)
    with _writer_lock("run-all", options):
        if resume is None:
            # One mode for both: the chain and `runs.mode` are built from the guard's
            # verdict, so an escalated run executes the full chain AND is recorded as
            # a full run. Recording one and executing the other would leave
            # `run_stages` describing a chain that never ran (S4.0, S5.2).
            effective = _guarded_mode(mode, options, allow_escalate=allow_escalate)
            chain = run_all_chain(effective, skip_ingest, source=source, path=delivery)
            _run_chain(chain, options, mode=effective)
            return
        plan = _resume(resume, options)
        try:
            chain = _resume_chain(
                plan, run_all_chain(plan.mode, skip_ingest, source=source, path=delivery)
            )
        except ErError as exc:
            raise typer.Exit(_refuse(resume, exc)) from exc
        # The run is re-entered, not re-minted: `RunContext` returns the existing
        # `runs` row to `running` and each re-executed stage updates its own row and
        # keeps its `seq`, so `(run_id, stage)` still holds exactly one row (S5.2).
        _run_chain(
            chain,
            replace(options, run_id=plan.run_id, config_hash=plan.config_hash),
            mode=plan.mode,
            model_version=plan.model_version,
        )


@app.command()
def correct(
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """The periodic correction pass that restores INV-EQ (S4.0, S4.5.6).

    Its chain is ``match --mode full --new-tf-snapshot`` -> ``reconcile`` ->
    ``assemble``, and it is the only caller allowed to pass ``--new-tf-snapshot``
    (D4). It never trains.
    """
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    _run_single("correct", options, mode="correction_pass")


@app.command("assert")
def assert_(
    action: Annotated[str, typer.Argument(help="add | remove | load.")],
    a: Annotated[str | None, typer.Option("--a", help="rec_a_key, for add.")] = None,
    b: Annotated[str | None, typer.Option("--b", help="rec_b_key, for add.")] = None,
    kind: Annotated[str | None, typer.Option("--kind", help="always | never, for add.")] = None,
    by: Annotated[str | None, typer.Option("--by", help="Steward, for add and remove.")] = None,
    note: Annotated[str | None, typer.Option("--note", help="Free text, for add.")] = None,
    assertion_id: Annotated[
        str | None, typer.Option("--assertion-id", help="Assertion to retract, for remove.")
    ] = None,
    assert_path: Annotated[
        Path | None, typer.Option("--path", help="Assertion file, for load.")
    ] = None,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Add, retract or bulk-load steward assertions (S4.4)."""
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    _run_single("assert", options, (action,))


@app.command()
def review(
    action: Annotated[str, typer.Argument(help="list | resolve.")],
    status: Annotated[str, typer.Option("--status", help="Filter, for list.")] = "open",
    limit: Annotated[int, typer.Option("--limit", help="Row cap, for list.")] = 100,
    review_id: Annotated[
        str | None, typer.Option("--review-id", help="Row to resolve, for resolve.")
    ] = None,
    as_: Annotated[
        str | None, typer.Option("--as", help="match | no_match | dismiss, for resolve.")
    ] = None,
    by: Annotated[str | None, typer.Option("--by", help="Steward, for resolve.")] = None,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """List or resolve the gray-band review queue (S4.3.5)."""
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    # `list` is the only read-only verb S4.7 names, so anything else takes the lock —
    # including a verb that does not exist. A misspelling that skipped the lock would
    # be a writer's refusal turned into an unlocked run by a typo.
    verb = "review list" if action == "list" else "review resolve"
    _run_single("review", options, (action,), command=verb)


@lake_app.command("maintain")
def lake_maintain(
    retain_days: Annotated[int, typer.Option("--retain-days", help="Snapshot retention.")] = 7,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Merge files and expire snapshots outside the retention window (S4.0)."""
    options = GlobalOptions.resolve(
        config_path=config, run_id=run_id, json_output=json_output, require_config=False
    )
    _run_single(
        "maintain",
        options,
        ("--retain-days", str(retain_days)),
        mode="maintain",
        command="lake maintain",
    )


@lake_app.command("reset")
def lake_reset(
    confirm_tenant: Annotated[
        str, typer.Option("--confirm-tenant", help="Must equal `tenant` in the config.")
    ],
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Destroy the namespace: drop the metadata schema and the DATA_PATH prefix.

    The config is REQUIRED here even though S4.0's env column names only lake env:
    the ``--confirm-tenant`` guard that keeps the destructive path behind a
    deliberate act compares against ``tenant`` in the document.
    """
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    _run_command(
        _ResetStage(confirm_tenant=confirm_tenant, args=("--confirm-tenant", confirm_tenant)),
        options,
        mode="reset",
        command="lake reset",
    )


def main() -> None:
    """Console-script entry point (``er = er.cli:main``)."""
    app()
