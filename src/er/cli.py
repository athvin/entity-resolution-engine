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

Out of scope by construction, and owned by later tickets: the lake connection and
the ``runs``/``run_stages`` writes (ER-023, which extends :func:`emit_stage_line`
rather than adding a second emitter), the advisory lock and ``--resume`` (ER-024),
and the ``--mode incremental`` config-drift guard (ER-034). The dbt subprocess and
the ``--vars`` payload moved to ``er.dbt_runner`` with ER-033; ``dbt_vars`` here is
that module's :func:`~er.dbt_runner.render_dbt_vars` under its ER-014 name, not a
second builder of the mapping.
"""

from __future__ import annotations

import json
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Annotated, Protocol, TextIO

import typer

from er.config.hashing import config_hash
from er.config.loader import ConfigValidationError, load_config
from er.config.schema import Config
from er.dbt_runner import render_dbt_vars
from er.entities.ids import IdFactory, UlidFactory
from er.errors import ErError, ErrorClass, ExitCode, StageFailure, exit_code_for

__all__ = [
    "COMMANDS",
    "GlobalOptions",
    "NoOpStage",
    "NotImplementedStage",
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

#: ``run_stages.status`` is ``{running, succeeded, failed}`` (S5). A stage that
#: exits ``10`` is *succeeded* — S4.0 makes "nothing to do" a successful no-op —
#: so the log line carries ``exit_code`` as well, since the status alone cannot
#: distinguish a ``0`` from a ``10``.
_STATUS_SUCCEEDED = "succeeded"
_STATUS_FAILED = "failed"


def _utc_now() -> str:
    """Now, as the ISO-8601 UTC instant the S5.2 log line carries."""
    return datetime.now(UTC).isoformat(timespec="milliseconds").replace("+00:00", "Z")


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


def emit_stage_line(record: Mapping[str, object], *, stream: TextIO | None = None) -> None:
    """Write one stage record as exactly one JSON line on stderr (S4 preamble, S5.2).

    THE emitter. ER-023 extends the record this is called with — snapshot range,
    promoted counters, the ``counters`` payload — rather than adding a second
    writer, because "exactly one line per stage" is only checkable if there is one
    place that writes it.

    ``--json`` is not a parameter here: it switches *stdout*, and moving this line
    would break the guarantee that a caller can pipe stdout and still read
    telemetry.
    """
    target = sys.stderr if stream is None else stream
    target.write(json.dumps(dict(record), separators=(",", ":"), ensure_ascii=False) + "\n")
    target.flush()


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


def _execute(stage: Stage, options: GlobalOptions, seq: int) -> _Outcome:
    """Run one stage and emit its S5.2 line, whatever the stage did."""
    started_at = _utc_now()
    error_class: ErrorClass | None = None
    error_detail: str | None = None
    try:
        code = stage.run(options)
    except ErError as exc:
        code = exit_code_for(exc)
        error_class = exc.error_class
        error_detail = exc.detail
    succeeded = code in (ExitCode.SUCCESS, ExitCode.NOTHING_TO_DO)
    status = _STATUS_SUCCEEDED if succeeded else _STATUS_FAILED
    record: dict[str, object] = {
        "run_id": options.run_id,
        "stage": stage.name,
        "seq": seq,
        "status": status,
        "exit_code": code,
        "started_at": started_at,
        "ended_at": _utc_now(),
        "config_hash": options.config_hash,
        "error_class": None if error_class is None else str(error_class),
        "error_detail": error_detail,
    }
    emit_stage_line(record)
    return _Outcome(
        stage=stage.name,
        exit_code=code,
        status=status,
        error_class=error_class,
        error_detail=error_detail,
        record=record,
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


def _run_single(name: str, options: GlobalOptions, args: Sequence[str] = ()) -> None:
    """Execute one stage as a standalone command and exit with its code."""
    outcome = _execute(_stage_for(name, args), options, seq=1)
    _report(outcome, options)
    raise typer.Exit(outcome.exit_code)


def _run_chain(stages: Sequence[Stage], options: GlobalOptions) -> None:
    """Execute a chain, apply the S4.0 propagation rule, and exit with its code.

    ``10`` never aborts: a stage with nothing to do leaves the downstream stages
    to discover the same thing for themselves. The first exit that is neither
    ``0`` nor ``10`` stops the chain and becomes the process's status.
    """
    final = int(ExitCode.SUCCESS)
    executed = 0
    for seq, stage in enumerate(stages, start=1):
        outcome = _execute(stage, options, seq=seq)
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
    force: Annotated[bool, typer.Option("--force", help="Recreate relations that exist.")] = False,
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Create the ddl.py-owned relations in the attached lake (S4.0, S5.0)."""
    options = GlobalOptions.resolve(
        config_path=config, run_id=run_id, json_output=json_output, require_config=False
    )
    _run_single("init", options, ("--force",) if force else ())


@app.command()
def doctor(
    config: ConfigOption = None,
    run_id: RunIdOption = None,
    json_output: JsonOption = False,
) -> None:
    """Assert every pinned version and runtime invariant (S2.1, T-DOCTOR-1)."""
    options = GlobalOptions.resolve(
        config_path=config, run_id=run_id, json_output=json_output, require_config=False
    )
    _run_single("doctor", options)


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
    """Append source deliveries to raw_records as version history (S4.1)."""
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    args = ["--source", source, "--path", str(path)]
    if full_refresh_keys:
        args.append("--full-refresh-keys")
    _run_single("ingest", options, args)


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
    _run_single("train", options, ("--if-changed",) if if_changed else ())


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
    """
    if not skip_ingest and (source is None or path is None):
        # Rejected before ANY stage runs, and exit 2 rather than 3: this is a
        # malformed invocation, not an unmet precondition of the lake (S4.0).
        raise typer.BadParameter(
            "--source and --path are required unless --skip-ingest is set",
            param_hint="--source/--path",
        )
    options = GlobalOptions.resolve(config_path=config, run_id=run_id, json_output=json_output)
    chain = run_all_chain(
        mode, skip_ingest, source=source, path=None if path is None else str(path)
    )
    _run_chain(chain, options)


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
    _run_single("correct", options)


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
    _run_single("review", options, (action,))


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
    _run_single("maintain", options, ("--retain-days", str(retain_days)))


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
    if options.config is not None and confirm_tenant != options.config.tenant:
        sys.stderr.write(
            f"tenant mismatch: --confirm-tenant {confirm_tenant!r} "
            f"is not {options.config.tenant!r}\n"
        )
        raise typer.Exit(int(ExitCode.CONFIG))
    _run_single("reset", options, ("--confirm-tenant", confirm_tenant))


def main() -> None:
    """Console-script entry point (``er = er.cli:main``)."""
    app()
