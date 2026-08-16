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

``er init``, ``er lake reset`` (ER-020), ``er ingest`` (ER-031), ``er lake maintain``
(ER-025), ``er train`` (ER-055) and ``er assert`` (ER-062) are the commands that do
real lake work here; the work itself lives in :mod:`er.lake.init`,
:mod:`er.ingest.landing`, :mod:`er.lake.maintain`, :mod:`er.matching.train` and
:mod:`er.review.assertions`, and this module owns only their S4.0 stdout and the exit
status derived from what they returned. Every other command is still a stub.

``er assert`` writes no ``run_stages`` row and that is S5's decision, not a shortcut:
the ``run_stages.stage`` enum has no ``assert`` value, so
:class:`~er.obs.runctx.RunContext` skips the row for it exactly as it does for every
name outside :data:`~er.lake.model.RUN_STAGES`. A steward action is recorded in
``assertions`` itself, whose lifecycle columns are what the next run reads.

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

Every command that reaches the lake now runs the S5.1 schema preflight (ER-035) in the
same window: after the writer lock, before the :class:`~er.obs.runctx.RunContext`. A
breaking difference between `ddl.py` and the live catalog exits ``3`` with the literal
``ERR_SCHEMA_BREAKING`` message and leaves no `runs` row, no `run_stages` row and no
committed snapshot behind. `er init` and `er lake reset` are exempt and both
exemptions are the spec's — see :data:`_PREFLIGHT_EXEMPT`. The same window also decides
``runs.rebuild_reason``: a `std_version` or `survivorship_version` bump makes the run a
planned rebuild, which is why it runs the full chain AND is recorded as one.

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
from er.lake.ddl import SchemaBreakingError, preflight_schema
from er.lake.ducklake import connect
from er.lake.env import MissingEnvError
from er.lake.init import init_lake, reset_lake
from er.lake.maintain import DEFAULT_RETAIN_DAYS, maintain
from er.lake.objectstore import ObjectStore
from er.matching.train import run_train_stage
from er.obs.logging import emit_stage_record
from er.obs.runctx import RunContext, StageRun
from er.resume import ResumePlan, read_resume_rows, resume_plan
from er.review.assertions import add_assertion, load_assertions_csv, retract_assertion
from er.versions import (
    MODE_CORRECTION_PASS,
    ModeDecision,
    RunFingerprint,
    check_mode_preconditions,
    last_successful_run,
    rebuild_reason_for,
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

#: The three sub-verbs S4.0 gives ``er assert``, as the user types them. A positional
#: argument rather than three commands because S4.0's table gives ``er assert`` ONE
#: row whose flags vary by verb, and "no flag beyond that table" is only checkable
#: while the flag set is declared once.
_ASSERT_VERBS: tuple[str, ...] = ("add", "remove", "load")

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

#: The two commands the S5.1 stage preflight does NOT run for, and both exemptions
#: are the spec's rather than a shortcut. `er init` classifies the very same
#: differences itself, before it applies anything (:func:`~er.lake.init.init_lake`),
#: and refusing it here would only move its exit `3` earlier while adding a second
#: place the same message is produced. `er lake reset` destroys the namespace: a
#: breaking difference is one of the states an operator resets *out of*, so refusing
#: to drop a drifted schema would strand them inside the failure.
_PREFLIGHT_EXEMPT: frozenset[str] = frozenset({"init", "lake reset"})


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
class _MaintainStage:
    """`er lake maintain`: reclaim files and snapshot history (S4.0, S3).

    The three calls, their order and the retention guard are
    :func:`~er.lake.maintain.maintain`'s; this class owns the S4.0 surface — the
    ``--retain-days`` flag, the three-count stdout line, and the counters on the
    `run_stages` row S5.2 puts them on.

    Unlike `er lake reset`, this is an ordinary writer: it takes the lock through
    :func:`_writer_lock` like every other mutating command, and its `runs` and
    `run_stages` rows are persisted, because maintenance neither creates nor
    destroys the relations they live in.
    """

    retain_days: int = DEFAULT_RETAIN_DAYS
    args: tuple[str, ...] = ()
    name: str = "maintain"
    stage_run: StageRun | None = None

    def bind(self, stage_run: StageRun) -> None:
        self.stage_run = stage_run

    def run(self, options: GlobalOptions) -> int:
        with connect() as connection:
            result = maintain(connection, self.retain_days)
        counts: dict[str, object] = {
            "files_merged": result.files_merged,
            "snapshots_expired": result.snapshots_expired,
            "files_deleted": result.files_deleted,
        }
        if self.stage_run is not None:
            # S4 declares no counter list for `maintain`, so these are free-form
            # names in the `counters` payload (S5.2) — the three counts the command
            # printed, recorded where a later run can read them back.
            for counter, value in counts.items():
                self.stage_run.counters.set(counter, value)
        _write_stdout(
            counts,
            ", ".join(f"{counter}={value}" for counter, value in counts.items()),
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


@dataclass
class _TrainStage:
    """`er train`: fit a model, register it, and make it the active one (S4.3.2).

    The stage owns the S4.0 surface — the ``--if-changed`` flag, the five-field stdout
    line, and the ``0`` / ``10`` split — and none of the lifecycle:
    :func:`~er.matching.train.run_train_stage` decides both, because the exit code
    follows from whether a version was allocated and nothing else here can tell.

    The object store is constructed here rather than inside the stage function so that
    there is one place the environment is read, and so that a caller with a store of
    its own — the integration suite's failing-upload arm — can supply one without the
    stage reaching for `ER_S3_*` behind it.
    """

    if_changed: bool = False
    args: tuple[str, ...] = ()
    name: str = "train"
    stage_run: StageRun | None = None

    def bind(self, stage_run: StageRun) -> None:
        self.stage_run = stage_run

    def run(self, options: GlobalOptions) -> int:
        if options.config is None or options.config_hash is None:
            # Unreachable through the command tree: S4.0 lists `ER_CONFIG` in this
            # command's required-env column, so `GlobalOptions.resolve` has already
            # exited 2 without a document. Stated rather than asserted so a stage
            # driven some other way fails loudly instead of registering a model whose
            # `config_hash` column would have to be invented.
            raise StageFailure("er train was invoked without a validated config")
        store = ObjectStore.from_env()
        with connect() as connection:
            result = run_train_stage(
                connection,
                options.config,
                store,
                run_id=options.run_id,
                config_hash=options.config_hash,
                if_changed=self.if_changed,
                stage_run=self.stage_run,
            )
        _write_stdout(result.manifest(), result.stdout_line(), options)
        return result.exit_code


@dataclass(frozen=True)
class _AssertStage:
    """`er assert`: the three steward verbs over the `assertions` relation (S4.4).

    Every verb prints S4.0's five fields — `assertion_id, rec_a_key, rec_b_key, kind,
    active` — and nothing else, which is why this is a :func:`_run_command` stage
    rather than a :func:`_run_single` one: the generic per-stage summary would be a
    second, differently shaped line in a ``--json`` stream S4.0 says holds only the
    command's output.

    The flag combinations are validated HERE, before :func:`~er.lake.ducklake.connect`
    is called, so a malformed invocation exits ``2`` without opening a connection
    (S4.0). They are not validated in the command function as well: one place decides
    which flags a verb needs, and a second would be free to disagree with it.

    The stage decides no exit code of its own beyond ``load``'s ``10``. A conflicting
    insert raises :class:`~er.review.assertions.AssertionConflict`, a bad key or kind
    raises :class:`~er.errors.ConfigError`, and :func:`~er.errors.exit_code_for`
    derives ``1`` and ``2`` from the S4.7 class each carries.
    """

    action: str
    a: str | None = None
    b: str | None = None
    kind: str | None = None
    by: str | None = None
    note: str | None = None
    assertion_id: str | None = None
    path: Path | None = None
    args: tuple[str, ...] = ()
    name: str = "assert"

    def _flag(self, value: str | None, flag: str) -> str:
        """``value``, or the S4.0 exit-``2`` refusal for a verb missing a flag."""
        if value is None:
            raise ConfigError(f"er assert {self.action}: {flag} is required (S4.0)")
        return value

    def run(self, options: GlobalOptions) -> int:
        if self.action == "add":
            a = self._flag(self.a, "--a")
            b = self._flag(self.b, "--b")
            kind = self._flag(self.kind, "--kind")
            by = self._flag(self.by, "--by")
            with connect() as connection:
                written = add_assertion(
                    connection, a=a, b=b, kind=kind, created_by=by, note=self.note
                )
            _write_stdout(written.manifest(), written.stdout_line(), options)
            return int(ExitCode.SUCCESS)
        if self.action == "remove":
            assertion_id = self._flag(self.assertion_id, "--assertion-id")
            by = self._flag(self.by, "--by")
            with connect() as connection:
                retracted = retract_assertion(connection, assertion_id, retracted_by=by)
            _write_stdout(retracted.manifest(), retracted.stdout_line(), options)
            return int(ExitCode.SUCCESS)
        if self.action == "load":
            if self.path is None:
                raise ConfigError("er assert load: --path is required (S4.0)")
            with connect() as connection:
                applied = load_assertions_csv(connection, self.path)
            for written in applied:
                _write_stdout(written.manifest(), written.stdout_line(), options)
            # S4.0's `10` is "nothing to do", and a file whose every row is already
            # present and active is exactly that: no row was written, and the lake is
            # in the state the file asks for.
            return int(ExitCode.SUCCESS if applied else ExitCode.NOTHING_TO_DO)
        raise ConfigError(
            f"er assert: unknown verb {self.action!r}; S4.0 declares {', '.join(_ASSERT_VERBS)}"
        )


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


def _preflight_schema(command: str) -> None:
    """Refuse a command whose lake has drifted breakingly from S5 (S5.1, S4.7).

    S5.1: "`er init` and every stage's preflight detect a breaking difference between
    `ddl.py` and the live catalog and abort with exit code `3` … No snapshot is
    committed." Called with the writer lock already held and BEFORE the
    :class:`~er.obs.runctx.RunContext` is entered, which is what makes that last clause
    true of the control flow: there is no `runs` row, no `run_stages` row and no
    committed snapshot at the moment it refuses.

    A process with no lake environment passes through, exactly as
    :func:`_last_successful_run` and :func:`_writer_lock` do: the S8.4 unit layer
    drives the command tree on a bare runner with no services, and a namespace nothing
    can reach has no schema to have drifted.

    Raises:
        typer.Exit: exit ``3``, after writing the literal S5.1 message to stderr.
            :class:`~er.lake.ddl.SchemaBreakingError` already carries the S4.7
            `precondition` class and the code that follows from it, so nothing here
            chooses either.
    """
    if command in _PREFLIGHT_EXEMPT:
        return
    try:
        with connect() as connection:
            preflight_schema(connection)
    except MissingEnvError:
        return
    except SchemaBreakingError as exc:
        # stderr, not stdout: the message is the operator's grep token, and a caller
        # piping the command's output must not have to parse a refusal out of it.
        sys.stderr.write(f"{exc}\n")
        raise typer.Exit(exit_code_for(exc)) from exc


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
    rebuild_reason: str | None = None,
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

    ``rebuild_reason`` is :func:`~er.versions.rebuild_reason_for`'s verdict and is
    non-``None`` only for a planned rebuild: a `std_version` or `survivorship_version`
    bump, or `er correct` (S5.1, S4.0). It is what puts the run outside T-INC-2's
    accounting, so it is decided once by the caller that also decided the run's mode
    rather than by each stage.
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
        rebuild_reason=rebuild_reason,
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
    rebuild_reason: str | None = None,
) -> None:
    """Execute one stage as a standalone command and exit with its code.

    ``typer.Exit`` is raised after the context closes, never inside it: an exception
    escaping the stage body is how :class:`~er.obs.runctx.RunContext` learns the run
    failed, and an ordinary exit code is not a failure.

    The S5.1 preflight runs between the lock and the context, which is the only place
    it can: after the lock, so the schema it reads cannot change underneath it, and
    before the context, so a refusal leaves no `run_stages` row behind.

    Args:
        command: the S4.0 command path, when it differs from the stage name — `er lake
            maintain` writes a `maintain` stage, and `er review` is two commands under
            one stage. It decides whether the writer lock is taken (S4.7) and whether
            the S5.1 preflight runs.
        rebuild_reason: `runs.rebuild_reason` for this invocation; `er correct` is the
            one standalone command that has one (S4.0).
    """
    path = name if command is None else command
    with _writer_lock(path, options):
        _preflight_schema(path)
        with _run_context(options, mode=mode, rebuild_reason=rebuild_reason) as run:
            outcome = _execute(_stage_for(name, args), options, run)
    _report(outcome, options)
    raise typer.Exit(outcome.exit_code)


def _run_command(
    stage: Stage, options: GlobalOptions, *, mode: str, command: str, persist: bool = False
) -> None:
    """Execute a stage that writes its OWN stdout, and exit with its code.

    Distinct from :func:`_run_single` in exactly one way, and it is a contract and
    not a preference: S4.0 gives `er init`, `er lake reset`, `er ingest` and `er lake
    maintain` a stdout of their own — one line per relation, the reset summary, the
    S4.1 manifest line, the three maintenance counts — so the per-stage summary
    :func:`_report` writes would be a second, differently shaped line in a ``--json``
    stream that S4.0 says holds only the command's output.

    The failure message is repeated as a bare stderr line beside the S5.2 record.
    The record is telemetry a caller parses; the line is what an operator reads, and
    for `er init` it is the literal S4.0 immutability message.

    ``persist`` is false for the two namespace lifecycle verbs and true for every
    real stage: `er init` creates the relations the rows go in and `er lake reset`
    drops them, while `er ingest` is an ordinary writer whose `runs` and `run_stages`
    rows are exactly what S5.2 asks for (see :func:`_run_context`).
    """
    with _writer_lock(command, options):
        _preflight_schema(command)
        with _run_context(options, mode=mode, persist=persist) as run:
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
    rebuild_reason: str | None = None,
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
    with _run_context(
        options, mode=mode, model_version=model_version, rebuild_reason=rebuild_reason
    ) as run:
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


def _current_fingerprint(options: GlobalOptions) -> RunFingerprint | None:
    """The fingerprint this invocation would be recorded under, or ``None``.

    ``None`` is unreachable through the command tree for the commands that ask —
    S4.0 lists `ER_CONFIG` in `run-all`'s and `er correct`'s required-env column, so
    :meth:`GlobalOptions.resolve` has already exited ``2`` without a document. It is
    stated rather than asserted so that a guard with nothing to compare cannot
    silently compare two ``None``s and call them equal.
    """
    document = options.config
    if document is None or options.config_hash is None:
        return None
    return RunFingerprint(
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


def _guarded_mode(
    mode: str,
    options: GlobalOptions,
    *,
    prior: RunFingerprint | None,
    allow_escalate: bool,
) -> str:
    """The S4.0 config-drift guard: the mode to run under, or the refusal it earns.

    Called with the writer lock already held and before the first stage runs, which is
    what makes a refusal an S4.7 `precondition` rather than a failed run: no `runs`
    row, no `run_stages` row and no committed snapshot exist yet to be left behind.

    `--mode full` and a namespace with no prior successful run both reach
    :func:`~er.versions.check_mode_preconditions` and are decided there, not short-cut
    here — one guard, one place it says yes.

    ``prior`` is passed in rather than read here because the caller reads it once and
    asks it two questions: this one, and what :func:`~er.versions.rebuild_reason_for`
    records the run as. Reading it twice would let a run be escalated for a version
    bump and recorded without the reason for it (S5.1).

    Raises:
        typer.Exit: exit `3` when an incremental run's fingerprint has drifted and
            ``--allow-escalate`` was not given, after naming every drifted field and
            both its values on stderr.
    """
    current = _current_fingerprint(options)
    if current is None:
        return mode
    decision = check_mode_preconditions(prior, current, mode, allow_escalate=allow_escalate)
    return _apply_mode_decision(decision)


def _prior_run(options: GlobalOptions) -> RunFingerprint | None:
    """The last successful run for this invocation's tenant, or ``None``.

    ``None`` for an invocation with no validated document, which has no tenant to key
    the query on — the same judgement :func:`_writer_lock` makes about the lock.
    """
    tenant = _tenant_of(options)
    return None if tenant is None else _last_successful_run(tenant)


def _rebuild_reason(options: GlobalOptions, prior: RunFingerprint | None) -> str | None:
    """`runs.rebuild_reason` for this `run-all` invocation (S5.1).

    Non-``None`` only for a planned rebuild: a `std_version` or `survivorship_version`
    bump against the last successful run. `--mode full` is included and that is
    deliberate — S5.1 ties the reason to the *bump*, not to how the run reached full
    mode, so a bump run explicitly as `--mode full` and one promoted there by
    ``--allow-escalate`` are recorded identically and are equally outside T-INC-2.
    """
    current = _current_fingerprint(options)
    return None if current is None else rebuild_reason_for(prior, current)


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

    Runs full-corpus EM over `int_std_records`, freezes term frequency under a new
    `tf_snapshot_id`, writes the settings JSON to
    ``{storage.model_uri_prefix}model_v{N}.json`` and inserts the `model_registry` row
    that supersedes the previous active one. A new `model_version` is allocated on
    every invocation unless ``--if-changed`` is passed.

    Exit codes (S4.0): ``0`` success; ``10`` with ``--if-changed`` when the active
    model already carries this ``(config_hash, corpus_snapshot)``; ``1`` EM failure;
    ``2`` an incomplete ``training:`` block.

    Never reached from ``er run-all`` or ``er correct``: training is always an
    explicit, separate invocation (S4.0).
    """
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    _run_command(
        _TrainStage(if_changed=if_changed, args=("--if-changed",) if if_changed else ()),
        options,
        mode="train",
        command="train",
        persist=True,
    )


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
        _preflight_schema("run-all")
        if resume is None:
            # Read once and asked twice: the guard decides whether this run may
            # proceed, and `rebuild_reason_for` decides what the run that proceeded is
            # recorded as. Two reads could answer the two questions differently and
            # leave a version-bump rebuild inside T-INC-2's accounting (S5.1).
            prior = _prior_run(options)
            # One mode for both: the chain and `runs.mode` are built from the guard's
            # verdict, so an escalated run executes the full chain AND is recorded as
            # a full run. Recording one and executing the other would leave
            # `run_stages` describing a chain that never ran (S4.0, S5.2).
            effective = _guarded_mode(mode, options, prior=prior, allow_escalate=allow_escalate)
            chain = run_all_chain(effective, skip_ingest, source=source, path=delivery)
            _run_chain(
                chain,
                options,
                mode=effective,
                rebuild_reason=_rebuild_reason(options, prior),
            )
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

    S4.0 gives it ``runs.rebuild_reason='correction_pass'`` unconditionally: the pass
    rebuilds by definition, so the reason does not depend on whether anything drifted,
    and the run is outside T-INC-2's accounting like every other planned rebuild
    (S5.1).
    """
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    current = _current_fingerprint(options)
    _run_single(
        "correct",
        options,
        mode=MODE_CORRECTION_PASS,
        rebuild_reason=(
            None
            if current is None
            else rebuild_reason_for(None, current, mode=MODE_CORRECTION_PASS)
        ),
    )


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
    """Add, retract or bulk-load steward assertions (S4.4).

    ``--a`` and ``--b`` are unordered: the pair is canonicalised to
    ``rec_a_key < rec_b_key`` on the way in (S5.0, D9), so passing them the wrong way
    round writes a canonical row rather than earning an error.

    Rows are never deleted. ``remove`` sets ``active=false`` and stamps
    ``retracted_by``/``retracted_at``, because the assertion delta between two runs is
    computed from those stamps (S4.5.1).

    Exit codes (S4.0): ``0`` success; ``1`` a conflicting insert — ``never`` and
    ``always`` cannot both be active for one pair, and S4.4 rejects the second at
    write time rather than ordering around it; ``2`` a malformed key or an unknown
    kind; ``10`` from ``load`` when every row of the file is already active.
    """
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    _run_command(
        _AssertStage(
            action=action,
            a=a,
            b=b,
            kind=kind,
            by=by,
            note=note,
            assertion_id=assertion_id,
            path=assert_path,
            args=(action,),
        ),
        options,
        mode=_MODE_STAGE,
        command="assert",
        persist=True,
    )


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
    retain_days: Annotated[
        int, typer.Option("--retain-days", help="Snapshot retention.")
    ] = DEFAULT_RETAIN_DAYS,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Merge files and expire snapshots outside the retention window (S4.0)."""
    options = GlobalOptions.resolve(
        config_path=config, run_id=run_id, json_output=json_output, require_config=False
    )
    _run_command(
        _MaintainStage(retain_days=retain_days, args=("--retain-days", str(retain_days))),
        options,
        mode="maintain",
        command="lake maintain",
        persist=True,
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
