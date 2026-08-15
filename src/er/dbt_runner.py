"""The one place dbt is invoked from Python (DesignDoc.md S4.0b, S4.2, S4.6, S6).

Two functions, and every dbt-backed stage uses both:

* :func:`render_dbt_vars` is THE builder of the ``--vars`` payload. The S6 document
  — never ``dbt_project.yml`` — is the single source of truth for ``std_version``,
  ``survivorship_version``, the ``sources`` column mapping and the
  ``standardization`` block, so the override always wins and the fallbacks in
  ``dbt_project.yml`` exist only so a bare ``dbt parse`` renders on a runner with
  no config document (S9.1). ``er.cli.dbt_vars`` is this function under its ER-014
  name, not a second implementation.
* :func:`run_dbt` owns argv construction, target selection, log capture and the
  exit-code mapping.

Three rules are enforced here rather than left to each caller:

* **No Python DuckDB connection spans a dbt invocation** (S4.0b). That is why the
  signature takes ``close_conn``/``reopen_conn`` callables instead of a connection:
  closing is not something a caller can be trusted to remember, and the ordering is
  then assertable without a lake. ``reopen_conn`` runs in a ``finally``, so a
  failing dbt run does not leave the stage without a connection.
* **No list of entity ids may ever travel as a var** (S4.6). At the 1m scale tens
  of thousands of 26-character ULIDs exceed Linux's 128 KiB per-argv-element limit
  and the invocation hard-fails with ``E2BIG``, so the touched set goes through the
  ``er_touched_entities`` relation and the payload is refused if it carries an id
  list or simply grows too large.
* **``--full-refresh`` is never implicit** (S4.2). The absence of ``--changed-only``
  widens the *selection*; only a planned S5.1 version-bump rebuild asks for a full
  refresh, and it asks explicitly.

dbt's own ``threads`` is pinned to ``1`` in ``profiles.yml`` (S4.0b) and is
deliberately never a command-line flag here.
"""

from __future__ import annotations

import json
import re
import subprocess
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from er.config.schema import Config
from er.errors import ConfigError, StageFailure

__all__ = [
    "ARTIFACTS_DIR",
    "CONFIG_SOURCED_VARS",
    "DBT_PROFILES_DIR",
    "DBT_PROJECT_DIR",
    "MAX_VARS_BYTES",
    "DbtResult",
    "ModelRun",
    "render_dbt_vars",
    "run_dbt",
]

#: ``--project-dir``. Relative, and resolved against the process working directory,
#: which is the repo root locally and ``/app`` in the image (S3, docker/Dockerfile).
DBT_PROJECT_DIR = "dbt"

#: ``--profiles-dir``. The profile lives inside the project rather than in
#: ``~/.dbt`` so the same path works in the image and on a runner (S9.1).
DBT_PROFILES_DIR = "dbt/profiles"

#: Where the captured stdout+stderr of an invocation lands (S4.7).
ARTIFACTS_DIR = Path("artifacts")

#: The keys :func:`render_dbt_vars` sources from the S6 document. ``extra`` may not
#: shadow any of them: a stage that could override ``std_version`` would make the
#: document stop being the single source of truth exactly where it matters most.
CONFIG_SOURCED_VARS: frozenset[str] = frozenset(
    {"std_version", "survivorship_version", "run_id", "sources", "standardization"}
)

#: Hard ceiling on the encoded ``--vars`` payload. Linux's ``MAX_ARG_STRLEN`` is
#: 128 KiB per argv element; a quarter of that is still an order of magnitude above
#: any legitimate payload (``configs/test.yaml``'s three sources render under 2 KiB)
#: and turns the S4.6 ``E2BIG`` failure into a refusal that names its own cause.
MAX_VARS_BYTES = 32 * 1024

# Crockford base32 without I, L, O and U — the alphabet `python-ulid` emits. Used
# only to recognise an id list that must not travel as a var (S4.6).
_ULID_RE = re.compile(r"[0-7][0-9A-HJKMNP-TV-Z]{25}")

# dbt writes this after every `run`, `build`, `test` and `seed`; `parse` does not.
_RUN_RESULTS = Path("target") / "run_results.json"


@dataclass(frozen=True)
class ModelRun:
    """One entry of dbt's ``run_results.json``, in the terms S5 records."""

    unique_id: str
    status: str
    rows_affected: int | None = None
    execution_time: float | None = None


@dataclass(frozen=True)
class DbtResult:
    """What one dbt invocation did.

    ``snapshot_start``/``snapshot_end`` are ``None`` unless a probe was supplied:
    the range is read from the Python connection, which by S4.0b is closed for the
    whole of the subprocess, so it is captured on either side of it and never
    during.
    """

    exit_code: int
    models: tuple[ModelRun, ...]
    snapshot_start: int | None
    snapshot_end: int | None
    log_path: Path


def render_dbt_vars(
    cfg: Config,
    run_id: str,
    extra: Mapping[str, object] | None = None,
) -> dict[str, object]:
    """The ``--vars`` payload for one dbt invocation (S6, S4.2, S4.6).

    Args:
        cfg: the validated S6 document.
        run_id: this run's ULID. Every invocation carries it, because the marts
            join ``er_touched_entities`` filtered on it (S4.6).
        extra: payloads a stage adds — the S4.2 blocking rules, say. Merged, never
            allowed to shadow a config-sourced key.

    Raises:
        ConfigError: ``extra`` shadows a config-sourced var, or the payload carries
            a list of entity ids (S4.6).
    """
    payload: dict[str, object] = {
        "std_version": cfg.versions.std_version,
        "survivorship_version": cfg.versions.survivorship_version,
        "run_id": run_id,
        # The whole SourceSpec, not a projection: S4.2's `stg_<source>` models read
        # the column mapping and S4.6's `source_priority` rule reads
        # `priority_rank`, and a projection here would be a second place to edit
        # every time a model starts reading another S6 source field.
        "sources": {name: source.model_dump() for name, source in cfg.sources.items()},
        # Exactly S6's three keys, consumed by `email_norm` and `phone_e164`.
        "standardization": cfg.standardization.model_dump(),
    }
    if extra:
        shadowed = sorted(key for key in extra if key in payload)
        if shadowed:
            raise ConfigError(
                f"dbt vars: {shadowed} are sourced from the config document and "
                f"cannot be overridden by an extra payload"
            )
        payload.update(extra)
    _reject_id_lists(payload)
    return payload


def run_dbt(
    command: str,
    select: str | None = None,
    vars: Mapping[str, object] | None = None,
    target: str = "lake",
    full_refresh: bool = False,
    close_conn: Callable[[], None] | None = None,
    reopen_conn: Callable[[], None] | None = None,
    *,
    snapshot_probe: Callable[[], int] | None = None,
    project_dir: str = DBT_PROJECT_DIR,
    profiles_dir: str = DBT_PROFILES_DIR,
    artifacts_dir: Path = ARTIFACTS_DIR,
    spawn: Callable[[Sequence[str]], subprocess.CompletedProcess[str]] | None = None,
) -> DbtResult:
    """Invoke dbt as a subprocess and return what it did (S4.0b, S4.2, S4.7).

    Args:
        command: the dbt subcommand — ``run``, ``build``, ``test``, ``seed``.
        select: ``--select``; omitted entirely when ``None``, which selects the
            whole project. Widening the selection never implies ``--full-refresh``.
        vars: the payload from :func:`render_dbt_vars`. ``--vars`` is always passed,
            because S6 wants the version vars on every invocation; ``None`` renders
            the empty object rather than dropping the flag.
        target: the ``profiles.yml`` output — ``lake`` in the pipeline, ``mem`` for
            a service-less parse (S9.1).
        full_refresh: append ``--full-refresh``. Only a planned S5.1 rebuild sets
            it (S4.2).
        close_conn: closes the Python DuckDB connection. Called BEFORE the
            subprocess is spawned (S4.0b).
        reopen_conn: reopens it. Called after the subprocess exits, on the failure
            path too.
        snapshot_probe: reads the lake's current snapshot. Called while the
            connection is open, so before ``close_conn`` and after ``reopen_conn``.
        project_dir: ``--project-dir``.
        profiles_dir: ``--profiles-dir``.
        artifacts_dir: root of the captured-log tree.
        spawn: the subprocess runner; the real one by default.

    Raises:
        ConfigError: the encoded ``vars`` payload is unsafe to pass as one argv
            element (S4.6).
        StageFailure: dbt exited non-zero. Exit ``1`` per S4.7, with the captured
            output on disk.
    """
    argv = [
        "dbt",
        command,
        "--project-dir",
        project_dir,
        "--profiles-dir",
        profiles_dir,
        "--target",
        target,
    ]
    if select is not None:
        argv += ["--select", select]
    # One argv element, no shell: the quotes S4.2 writes around the JSON are shell
    # quoting, and adding them here would make dbt parse a leading `'`.
    argv += ["--vars", _encode_vars(vars)]
    if full_refresh:
        argv.append("--full-refresh")

    run = spawn if spawn is not None else _spawn
    snapshot_start = None if snapshot_probe is None else snapshot_probe()
    if close_conn is not None:
        close_conn()
    try:
        completed = run(argv)
    finally:
        # In `finally` so a dbt failure — or a spawn that never returns a status —
        # still leaves the stage with the connection S4.0b says it must reopen.
        if reopen_conn is not None:
            reopen_conn()
    snapshot_end = None if snapshot_probe is None else snapshot_probe()

    run_id = vars.get("run_id") if vars is not None else None
    log_path = _write_log(
        artifacts_dir,
        command=command,
        run_id=run_id if isinstance(run_id, str) else None,
        argv=argv,
        completed=completed,
    )
    if completed.returncode != 0:
        # S4.7 classifies a dbt failure as `data` — an unparsable source row, a
        # uniqueness test failure — which exits 1. A caller with a better diagnosis
        # re-raises with its own class; guessing one from an exit status would not
        # be a better one.
        raise StageFailure(
            f"dbt {command} exited {completed.returncode}; captured output: {log_path}"
        )
    return DbtResult(
        exit_code=completed.returncode,
        models=_read_run_results(Path(project_dir)),
        snapshot_start=snapshot_start,
        snapshot_end=snapshot_end,
        log_path=log_path,
    )


def _encode_vars(variables: Mapping[str, object] | None) -> str:
    """The ``--vars`` argv element: one compact, key-sorted JSON object.

    Sorted so two invocations of the same payload produce byte-identical argv, which
    is what makes an invocation reproducible from a log line.
    """
    payload = dict(variables) if variables is not None else {}
    _reject_id_lists(payload)
    encoded = json.dumps(payload, separators=(",", ":"), sort_keys=True, ensure_ascii=False)
    size = len(encoded.encode("utf-8"))
    if size > MAX_VARS_BYTES:
        raise ConfigError(
            f"dbt vars: payload is {size} bytes, over the {MAX_VARS_BYTES}-byte limit; "
            f"a per-entity or per-record collection belongs in a relation, not in argv (S4.6)"
        )
    return encoded


def _reject_id_lists(payload: Mapping[str, object]) -> None:
    """Refuse a payload carrying a list of ULIDs (S4.6).

    The ban is structural rather than by key name: the failure it prevents is
    ``E2BIG`` at the 1m scale, and a var called anything at all whose value is a
    list of entity ids produces it.
    """
    offenders = sorted(key for key, value in payload.items() if _holds_id_list(value))
    if offenders:
        raise ConfigError(
            f"dbt vars: {offenders} carry a list of ULIDs; the touched set travels "
            f"through er_touched_entities, never as a var (S4.6)"
        )


def _holds_id_list(value: object) -> bool:
    """Whether ``value`` is, or contains, a list with a ULID-shaped string in it."""
    if isinstance(value, str):
        return False
    if isinstance(value, Mapping):
        return any(_holds_id_list(item) for item in value.values())
    if isinstance(value, Sequence):
        if any(isinstance(item, str) and _ULID_RE.fullmatch(item) for item in value):
            return True
        return any(_holds_id_list(item) for item in value)
    return False


def _spawn(argv: Sequence[str]) -> subprocess.CompletedProcess[str]:
    """Run ``argv`` to completion, capturing both streams.

    No shell, so nothing in the ``--vars`` JSON is ever word-split or expanded, and
    ``check=False`` because the exit status is mapped by the caller rather than
    raised as a ``CalledProcessError`` the S4.7 taxonomy would have to unwrap.
    """
    return subprocess.run(list(argv), capture_output=True, text=True, check=False)


def _write_log(
    artifacts_dir: Path,
    *,
    command: str,
    run_id: str | None,
    argv: Sequence[str],
    completed: subprocess.CompletedProcess[str],
) -> Path:
    """Persist the invocation and both captured streams, and return the path.

    Written on the success path too: dbt's per-model row counts are in it, and a
    log that only exists after a failure is a log nobody can diff against a run
    that worked. The name never collides, so a second invocation under one
    ``run_id`` — ``standardize`` then ``assemble`` — cannot overwrite the first.
    """
    directory = artifacts_dir / "dbt"
    directory.mkdir(parents=True, exist_ok=True)
    stem = command if run_id is None else f"{run_id}-{command}"
    path = directory / f"{stem}.log"
    seq = 2
    while path.exists():
        path = directory / f"{stem}.{seq}.log"
        seq += 1
    path.write_text(
        f"$ {' '.join(argv)}\nexit {completed.returncode}\n"
        f"--- stdout\n{completed.stdout or ''}\n--- stderr\n{completed.stderr or ''}\n",
        encoding="utf-8",
    )
    return path


def _read_run_results(project_dir: Path) -> tuple[ModelRun, ...]:
    """The per-model results of the invocation that just finished.

    Empty when the file is absent or unreadable: ``dbt parse`` writes none, and a
    successful invocation is not retroactively a failure because its artifact could
    not be read.
    """
    path = project_dir / _RUN_RESULTS
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return ()
    if not isinstance(document, Mapping):
        return ()
    results = document.get("results")
    if not isinstance(results, list):
        return ()
    runs: list[ModelRun] = []
    for entry in results:
        if not isinstance(entry, Mapping):
            continue
        response = entry.get("adapter_response")
        rows = response.get("rows_affected") if isinstance(response, Mapping) else None
        execution_time = entry.get("execution_time")
        runs.append(
            ModelRun(
                unique_id=str(entry.get("unique_id", "")),
                status=str(entry.get("status", "")),
                rows_affected=int(rows) if isinstance(rows, int) else None,
                execution_time=float(execution_time)
                if isinstance(execution_time, int | float)
                else None,
            )
        )
    return tuple(runs)
