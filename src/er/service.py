"""In-process facade over the S4.0 orchestration, for hosting processes.

The CLI remains the contract's face (S4.0); this module gives an embedding
process — a job runner, a service — the same sequence ``er run-all`` and
``er correct`` execute (writer lock → pending-correction gate → schema
preflight → drift guard → chain), returning a typed :class:`RunOutcome`
instead of a process exit. Exit codes are still derived, never chosen: every
status here comes through :func:`er.errors.exit_code_for` or out of
:func:`er.cli._execute_chain`, exactly as the CLI's do.

What is reused and what is mirrored is deliberate. The chain builders
(:func:`er.cli.run_all_chain`, :func:`er.cli.correction_chain`), the executor
(:func:`er.cli._execute_chain`) and the resume slicing are the CLI's own — one
propagation rule, one place it is stated. The lock window and the drift guard
are *mirrored* from ``er.cli`` rather than imported, because the CLI renders
their refusals as ``typer.Exit`` after the class has been collapsed into a
status; a hosting process needs the S4.7 class itself (``lock_conflict`` is
retryable, ``precondition`` is not) to decide what happens next. Hoisting the
CLI onto this module so the mirror disappears is the intended follow-up, noted
here so the duplication reads as a step and not a fork.

A refusal never writes: the lock is tried before any ``runs`` row is minted,
in the same order as the CLI (T-CONC-1), because both call the same primitives
in the same sequence.
"""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass, replace
from pathlib import Path

from er.cli import (
    GlobalOptions,
    _execute_chain,
    _Outcome,
    _resume_chain,
    _TrainStage,
    run_all_chain,
)
from er.config.hashing import config_hash
from er.config.loader import ConfigValidationError, load_config
from er.entities.ids import IdFactory, UlidFactory
from er.errors import (
    ConfigError,
    ErError,
    ExitCode,
    PreconditionFailure,
    classify,
    exit_code_for,
)
from er.ingest.sources import adapter_for
from er.lake.catalog import tenant_lock
from er.lake.ddl import preflight_schema
from er.lake.ducklake import connect, invocation_session
from er.lake.env import EnvError, MissingEnvError
from er.lake.model import REBUILD_REASONS, SCHEMA_QUALIFIER
from er.lake.model_registry import active_model, find_active_model
from er.resume import ResumePlan, read_resume_rows, resume_plan
from er.versions import (
    MODE_CORRECTION_PASS,
    MODE_FULL,
    MODE_INCREMENTAL,
    RunFingerprint,
    check_mode_preconditions,
    last_successful_run,
    rebuild_reason_for,
)

__all__ = [
    "RunOutcome",
    "StageOutcome",
    "run_correction",
    "run_pipeline",
    "run_training",
]

#: The exceptions a refusal is built from: the classified taxonomy plus the two
#: pre-taxonomy families that carry a bare ``code`` attribute instead
#: (:func:`er.errors.exit_code_for` honours both by the same convention).
_REFUSABLE = (ErError, ConfigValidationError, EnvError)


@dataclass(frozen=True)
class StageOutcome:
    """One executed stage, in the terms `run_stages` records it (S5.2)."""

    stage: str
    exit_code: int
    status: str
    error_class: str | None
    error_detail: str | None


@dataclass(frozen=True)
class RunOutcome:
    """What one invocation did: the S4.0 status plus the S4.7 class behind it.

    ``stages`` is empty for a refusal — a refused writer writes nothing, runs
    nothing, and its ``error_class`` is the refusal's own (``lock_conflict``,
    ``precondition``, ``config``), which is exactly the value a caller's retry
    policy branches on.
    """

    run_id: str
    mode: str
    exit_code: int
    error_class: str | None
    error_detail: str | None
    stages: tuple[StageOutcome, ...]


def _stage_outcome(outcome: _Outcome) -> StageOutcome:
    return StageOutcome(
        stage=outcome.stage,
        exit_code=outcome.exit_code,
        status=outcome.status,
        error_class=None if outcome.error_class is None else outcome.error_class.value,
        error_detail=outcome.error_detail,
    )


def _refusal(run_id: str, mode: str, exc: BaseException) -> RunOutcome:
    return RunOutcome(
        run_id=run_id,
        mode=mode,
        exit_code=exit_code_for(exc),
        error_class=classify(exc).value,
        error_detail=str(exc),
        stages=(),
    )


def _completed(run_id: str, mode: str, final: int, outcomes: list[_Outcome]) -> RunOutcome:
    failed = next(
        (
            outcome
            for outcome in reversed(outcomes)
            if outcome.exit_code not in (int(ExitCode.SUCCESS), int(ExitCode.NOTHING_TO_DO))
        ),
        None,
    )
    return RunOutcome(
        run_id=run_id,
        mode=mode,
        exit_code=final,
        error_class=None
        if failed is None or failed.error_class is None
        else failed.error_class.value,
        error_detail=None if failed is None else failed.error_detail,
        stages=tuple(_stage_outcome(outcome) for outcome in outcomes),
    )


def _options(
    config_path: Path | None, run_id: str | None, id_factory: IdFactory | None
) -> GlobalOptions:
    """The CLI's eager resolution (S4.0): validate the document before anything else."""
    document = load_config(config_path)
    factory = id_factory if id_factory is not None else UlidFactory()
    return GlobalOptions(
        config=document,
        config_hash=config_hash(document),
        run_id=run_id if run_id else factory.new(),
        json_output=True,
    )


@contextmanager
def _writer_window(options: GlobalOptions, *, resume_run_id: str | None) -> Iterator[None]:
    """The S4.0b window in the CLI's order: lock, session, correction gate, preflight.

    Mirrors :func:`er.cli._writer_lock` + :func:`er.cli._preflight_schema` for the two
    always-mutating commands this module hosts, with one difference: refusals leave as
    the classified exception rather than a ``typer.Exit``, so the caller keeps the
    S4.7 class. A missing lake environment passes through exactly as it does in the
    CLI — the S8.4 unit layer drives this window on a bare runner with no services.
    """
    with ExitStack() as held:
        tenant = None if options.config is None else options.config.tenant
        if tenant is not None:
            try:
                held.enter_context(tenant_lock(tenant, run_id=options.run_id))
            except MissingEnvError:
                pass
        held.enter_context(invocation_session())
        from er.matching.correction import assert_no_pending_correction

        try:
            with connect() as connection:
                assert_no_pending_correction(
                    connection, resume_run_id=resume_run_id, assertion_repair=False
                )
        except MissingEnvError:
            pass
        try:
            with connect() as connection:
                preflight_schema(connection)
        except MissingEnvError:
            pass
        yield


def _guard(options: GlobalOptions, mode: str, allow_escalate: bool) -> tuple[str, str | None]:
    """The S4.0 drift guard plus the S5.1 rebuild verdict, off one fingerprint read.

    Mirrors the CLI's read-once-ask-twice rule: the prior fingerprint answers both
    "may this mode run" and "what is the run recorded as", so a version-bump rebuild
    cannot be escalated without also being recorded as one.
    """
    document = options.config
    if document is None or options.config_hash is None:
        return mode, None
    model_version = None
    try:
        with connect() as connection:
            active = find_active_model(connection)
        if active is not None:
            model_version = active.model_version
    except MissingEnvError:
        pass
    current = RunFingerprint(
        config_hash=options.config_hash,
        model_version=model_version,
        std_version=document.versions.std_version,
        survivorship_version=document.versions.survivorship_version,
    )
    try:
        with connect() as connection:
            prior = last_successful_run(connection, document.tenant)
    except MissingEnvError:
        prior = None
    decision = check_mode_preconditions(prior, current, mode, allow_escalate=allow_escalate)
    if decision.refused:
        raise PreconditionFailure(decision.message)
    return decision.mode, rebuild_reason_for(prior, current)


def _resume_plan_for(options: GlobalOptions, run_id: str) -> ResumePlan:
    """The S4.7 resume plan, with the CLI's model-still-active precondition."""
    if options.config_hash is None:
        raise ConfigError("resume needs the S6 document the run was hashed from")
    with connect() as connection:
        rows = read_resume_rows(connection, run_id)
        plan = resume_plan(rows, options.config_hash)
        active = find_active_model(connection)
        if plan.model_version is not None and (
            active is None or active.model_version != plan.model_version
        ):
            raise PreconditionFailure(
                "ERR_MODEL_VERSION_CHANGED: "
                f"cannot resume {run_id}: recorded model "
                f"{plan.model_version!r} is no longer active",
            )
    return plan


def run_pipeline(
    *,
    mode: str,
    config_path: Path | None = None,
    run_id: str | None = None,
    source: str | None = None,
    path: str | None = None,
    skip_ingest: bool = False,
    allow_escalate: bool = False,
    resume: str | None = None,
    reason: str | None = None,
    id_factory: IdFactory | None = None,
) -> RunOutcome:
    """``er run-all`` as a call: one run_id, the S4.0 chain, a typed outcome.

    Raises:
        ValueError: ``mode`` is neither ``incremental`` nor ``full`` — a caller
            bug, exactly as :func:`er.cli.run_all_chain` treats it, never a
            refusal outcome.
    """
    if mode not in (MODE_INCREMENTAL, MODE_FULL):
        raise ValueError(f"unknown run-all mode: {mode!r}")
    try:
        options = _options(config_path, run_id, id_factory)
    except _REFUSABLE as exc:
        return _refusal(run_id or "", mode, exc)
    try:
        if resume is None and not skip_ingest and (source is None or path is None):
            raise ConfigError("source and path are required unless skip_ingest is set")
        if reason is not None and reason not in REBUILD_REASONS:
            raise ConfigError(f"unknown rebuild reason: {reason!r}")
        if source is not None and path is not None and options.config is not None:
            adapter_for(options.config, source, Path(path))
    except _REFUSABLE as exc:
        return _refusal(options.run_id, mode, exc)
    try:
        with _writer_window(options, resume_run_id=None):
            if resume is None:
                effective, derived = _guard(options, mode, allow_escalate)
                chain = run_all_chain(
                    effective, skip_ingest, source=source, path=path, reason=reason
                )
                final, outcomes = _execute_chain(
                    chain,
                    options,
                    mode=effective,
                    rebuild_reason=reason if reason is not None else derived,
                )
                return _completed(options.run_id, effective, final, outcomes)
            plan = _resume_plan_for(options, resume)
            chain = _resume_chain(
                plan, run_all_chain(plan.mode, skip_ingest, source=source, path=path)
            )
            resumed = replace(options, run_id=plan.run_id, config_hash=plan.config_hash)
            final, outcomes = _execute_chain(
                chain, resumed, mode=plan.mode, model_version=plan.model_version
            )
            return _completed(plan.run_id, plan.mode, final, outcomes)
    except _REFUSABLE as exc:
        return _refusal(options.run_id, mode, exc)


def run_training(
    *,
    config_path: Path | None = None,
    run_id: str | None = None,
    if_changed: bool = False,
    id_factory: IdFactory | None = None,
) -> RunOutcome:
    """``er train`` as a call: fit, register and activate a model (S4.3.2).

    Training is never chained (S4.0: ``run-all`` and ``correct`` never train), so
    this is a one-stage invocation under the same lock window as every other
    mutating command. ``if_changed`` maps to ``--if-changed``: exit ``10`` when
    the active model already carries this ``(config_hash, corpus_snapshot)``.
    """
    try:
        options = _options(config_path, run_id, id_factory)
    except _REFUSABLE as exc:
        return _refusal(run_id or "", "train", exc)
    try:
        with _writer_window(options, resume_run_id=None):
            stage = _TrainStage(if_changed=if_changed, args=("--if-changed",) if if_changed else ())
            final, outcomes = _execute_chain([stage], options, mode="train")
            return _completed(options.run_id, "train", final, outcomes)
    except _REFUSABLE as exc:
        return _refusal(options.run_id, "train", exc)


def run_correction(
    *,
    config_path: Path | None = None,
    run_id: str | None = None,
    resume: str | None = None,
    id_factory: IdFactory | None = None,
) -> RunOutcome:
    """``er correct`` as a call: refresh frozen TF and re-resolve, never train.

    Raises:
        ValueError: ``run_id`` and ``resume`` are both given and disagree — the
            CLI's ``--run-id must equal --resume`` usage error, a caller bug.
    """
    if resume is not None and run_id is not None and resume != run_id:
        raise ValueError("run_id must equal resume when both are provided")
    try:
        options = _options(config_path, resume or run_id, id_factory)
    except _REFUSABLE as exc:
        return _refusal(resume or run_id or "", MODE_CORRECTION_PASS, exc)
    try:
        with _writer_window(options, resume_run_id=options.run_id):
            with connect() as connection:
                active_model(connection)
            if resume is None:
                with connect() as connection:
                    existing = connection.execute(
                        f"SELECT status FROM {SCHEMA_QUALIFIER}.runs WHERE run_id=?",
                        [options.run_id],
                    ).fetchone()
                if existing is not None:
                    raise PreconditionFailure(
                        "correction run already exists; use --resume for an unfinished run"
                    )
            from er.cli import correction_chain

            chain = correction_chain()
            model_version = None
            if resume is not None:
                plan = _resume_plan_for(options, resume)
                if plan.mode != MODE_CORRECTION_PASS:
                    raise PreconditionFailure("run is not a correction")
                chain = chain[
                    next(i for i, stage in enumerate(chain) if stage.name == plan.resume_from) :
                ]
                model_version = plan.model_version
            final, outcomes = _execute_chain(
                chain,
                options,
                mode=MODE_CORRECTION_PASS,
                model_version=model_version,
                rebuild_reason="correction_pass",
            )
            return _completed(options.run_id, MODE_CORRECTION_PASS, final, outcomes)
    except _REFUSABLE as exc:
        return _refusal(options.run_id, MODE_CORRECTION_PASS, exc)
