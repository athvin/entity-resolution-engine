"""`er doctor`: the runtime assertion of S2.1 and T-DOCTOR-1 (DesignDoc.md S4.0).

S2.1's pins are only real if something asserts them where the code actually runs, and
S13 names this command as the mitigation for version skew. So the check table is
**derived from** :data:`~er.versions.PINS` rather than hand-listed: S2.1's last rule is
that "a component with no row is not asserted and therefore is not pinned", and a
second, hand-maintained list here would let a spec row drift out of the doctor's reach
without anything going red. :func:`build_checks` refuses to build a table for a pin it
has no probe rule for, so adding an S2.1 row that names `er doctor` fails at import
until a probe exists for it.

Four constraints an implementer will otherwise get wrong, each of them the spec's
decision rather than this module's:

* **Exit `1`, never `3`.** A pin mismatch is a check failure under the S4.0 exit-code
  table; `3` is reserved there for the five named precondition failures (S2.1 says so
  in as many words).
* **The `postgres` extension is asserted under its *registered* name
  `postgres_scanner`.** `INSTALL postgres` is the install name, but
  ``duckdb_extensions()`` reports the extension as ``postgres_scanner`` and files
  ``postgres`` under ``aliases`` — so a check filtered on the install name matches no
  row, finds nothing to disagree with, and passes. The registered name comes from
  :class:`~er.versions.ExtensionPin`, which exists for exactly this asymmetry.
* **No `runs` / `run_stages` row, and no advisory lock.** ``run_stages.stage`` has no
  ``doctor`` value in the S5 enum, and the doctor mutates nothing; it is already in
  :data:`~er.cli.READ_ONLY_COMMANDS`. That is why :mod:`er.cli` runs it outside a
  :class:`~er.obs.runctx.RunContext` instead of as a :class:`~er.cli.Stage`.
* **No Python DuckDB connection spans a dbt subprocess** (S4.0b). The dbt probe closes
  the lake connection before it spawns; the next probe that needs one reopens it. That
  is what :meth:`DoctorContext.close_lake` is for, and it is why the connection is
  opened lazily rather than held for the length of the run.

**Every check runs.** A probe that raises produces a failed row carrying the exception
text, never a traceback that hides the twenty checks after it — the command's contract
is a table with one line per check plus an exit code, and a table that stops at the
first bad row is worth less than no table.

Two probes deviate from the letter of S2.1's *Asserted by* cell, because the container
S9.1 runs the doctor inside cannot supply what that cell names. Both are documented at
their probe: `uv` (:func:`_uv_version`) and dbt-duckdb (:func:`_dbt_adapter_version`).
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import AbstractContextManager, ExitStack
from dataclasses import dataclass, field
from pathlib import Path
from typing import Final

import duckdb
import yaml
from botocore.exceptions import ClientError

from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR
from er.lake.catalog import catalog_connect, server_version
from er.lake.ducklake import LAKE_ALIAS, connect
from er.lake.env import require_env
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.objectstore import ObjectStore
from er.versions import EXTENSION_PINS, IMAGE_PINS, PINS, ExtensionPin, Pin, installed_version

__all__ = [
    "CHECKS",
    "ERROR_PREFIX",
    "OK",
    "RUNTIME_CHECK_NAMES",
    "CheckResult",
    "DoctorCheck",
    "DoctorContext",
    "MissingProbeError",
    "build_checks",
    "check_lines",
    "check_record",
    "exit_code",
    "extension_version",
    "run_checks",
]

#: What a probe returns when the thing it asserts is present and correct. Behavioural
#: rows — the ones S2.1 assigns a round-trip rather than a version string — compare
#: this against itself, so their diagnosis lives in the ``actual`` of a *failed* row.
OK: Final[str] = "ok"

#: The prefix a failed probe's ``actual`` carries. Spelled once, because it is the
#: token an operator greps a CI log for.
ERROR_PREFIX: Final[str] = "error: "

#: The six T-DOCTOR-1 runtime assertions, in the order the doctor prints them. They
#: follow the pin rows, so the table reads S2.1 first and behaviour second.
RUNTIME_CHECK_NAMES: Final[tuple[str, ...]] = (
    "lake snapshots",
    "extensions loaded",
    "dbt target database",
    "data path round-trip",
    "catalog round-trip",
    "no __splink__ relations",
)

#: The object the DATA_PATH round-trip writes and reads back. Under
#: ``$ER_LAKE_DATA_PATH`` and therefore inside the namespace being checked, so a doctor
#: run cannot touch another tenant's prefix; the name is fixed, so a run killed between
#: the write and the delete leaves at most one of these behind.
_PROBE_OBJECT: Final[str] = "_er_doctor_probe"

#: What the round-trip writes. Non-empty, because a zero-byte object round-trips
#: successfully through an object store that has lost its contents.
_PROBE_PAYLOAD: Final[bytes] = b"er doctor T-DOCTOR-1"

#: S4.0b keeps Splink's intermediates in the in-memory database, so a relation under
#: this prefix in the lake means a scratch table was committed to DuckLake (M17).
_SPLINK_PREFIX: Final[str] = "__splink__"

#: Every relation in the attached lake, tables and views alike. Views are included
#: because the `__splink__` check asserts an *absence*, and an absence proved against
#: half the namespace is not proved at all.
_RELATIONS_SQL: Final[str] = """
SELECT count(*) FROM (
    SELECT database_name, schema_name, table_name AS relation_name FROM duckdb_tables()
    UNION ALL
    SELECT database_name, schema_name, view_name  AS relation_name FROM duckdb_views()
)
WHERE database_name = ? AND schema_name = ? AND relation_name LIKE ?
"""


class MissingProbeError(RuntimeError):
    """An S2.1 row names `er doctor` and this module has no probe for it.

    Raised while :data:`CHECKS` is being built, so the failure is an import error at
    the first `er` invocation rather than a component that is silently not asserted.
    """

    def __init__(self, component: str) -> None:
        super().__init__(
            f"S2.1 row {component!r} is asserted by `er doctor` and has no probe in "
            f"er.doctor; add one, or clear `asserted_by_doctor` in er.versions"
        )
        self.component = component


@dataclass
class DoctorContext:
    """The resources the probes share, opened on demand and closed together.

    Lazy rather than eager, for two reasons that are both S4.0b's. A connection held
    across the dbt subprocess would break the connection model, so the dbt probe closes
    it and a later probe reopens it. And the version rows, which need nothing at all,
    must still print on a machine with no lake — which they cannot do if opening the
    lake is the first thing the command does.

    Both factories are injectable so the unit layer can drive a probe without a
    substrate (S8.1: unit tests run on a bare runner with no services).
    """

    connect_lake: Callable[[], AbstractContextManager[duckdb.DuckDBPyConnection]] = connect
    build_store: Callable[[], ObjectStore] = ObjectStore.from_env

    _stack: ExitStack | None = field(default=None, repr=False)
    _connection: duckdb.DuckDBPyConnection | None = field(default=None, repr=False)
    _store: ObjectStore | None = field(default=None, repr=False)

    def lake(self) -> duckdb.DuckDBPyConnection:
        """The S4.0b connection, opened now if this run has not needed one yet."""
        if self._connection is None:
            # `connect()` is a context manager; the stack is what lets the connection
            # outlive the call that opened it and still be closed exactly once.
            stack = ExitStack()
            self._connection = stack.enter_context(self.connect_lake())
            self._stack = stack
        return self._connection

    def close_lake(self) -> None:
        """Close the lake connection if one is open (S4.0b, before a dbt subprocess).

        Idempotent: both the dbt probe and :func:`run_checks`'s ``finally`` call it,
        and the second call must not fail because the first already ran.
        """
        stack = self._stack
        self._stack = None
        self._connection = None
        if stack is not None:
            stack.close()

    def store(self) -> ObjectStore:
        """The S3 client, built once per run."""
        if self._store is None:
            self._store = self.build_store()
        return self._store


@dataclass(frozen=True, slots=True)
class DoctorCheck:
    """One row of the doctor's table: what is asserted, and how it is read.

    ``expected`` is a literal rather than a callable because every value it can take
    is known before the process starts — an S2.1 pin, :data:`OK`, or the lake alias
    S7.2 makes a constant. The environment-dependent half is always ``probe``.
    """

    name: str
    expected: str
    probe: Callable[[DoctorContext], str]


@dataclass(frozen=True, slots=True)
class CheckResult:
    """What one check found, in the four columns S4.0's stdout column names."""

    name: str
    expected: str
    actual: str
    ok: bool

    @property
    def verdict(self) -> str:
        """``pass`` or ``fail`` — the word the check line and the JSONL both carry."""
        return "pass" if self.ok else "fail"


# --- probes ---------------------------------------------------------------------


def _python_version(_: DoctorContext) -> str:
    """``sys.version_info[:2]``, as S2.1 writes the interpreter pin (T-DOCTOR-1)."""
    return f"{sys.version_info[0]}.{sys.version_info[1]}"


def _distribution_probe(distribution: str) -> Callable[[DoctorContext], str]:
    """A probe reading one distribution's version from the metadata database.

    Absence is data, not an exception (:func:`~er.versions.installed_version`): a
    missing package is a failed row, and the rows after it still print.
    """

    def probe(_: DoctorContext) -> str:
        installed = installed_version(distribution)
        return f"{ERROR_PREFIX}{distribution} is not installed" if installed is None else installed

    return probe


def extension_version(connection: duckdb.DuckDBPyConnection, pin: ExtensionPin) -> str:
    """The ``extension_version`` ``duckdb_extensions()`` reports for ``pin``.

    Filtered on :attr:`~er.versions.ExtensionPin.registered_name`, which for the
    `postgres` extension is ``postgres_scanner`` (S2.1). Filtering on the install name
    matches zero rows, and a check that matches zero rows asserts nothing while
    reporting success — the exact failure S2.1 spells out.
    """
    row = connection.execute(
        "SELECT extension_version FROM duckdb_extensions() WHERE extension_name = ?",
        [pin.registered_name],
    ).fetchone()
    if row is None:
        return f"{ERROR_PREFIX}duckdb_extensions() reports no {pin.registered_name} row"
    return str(row[0])


def _extension_probe(pin: ExtensionPin) -> Callable[[DoctorContext], str]:
    def probe(context: DoctorContext) -> str:
        return extension_version(context.lake(), pin)

    return probe


def _uv_version(_: DoctorContext) -> str:
    """The `uv` that produced this environment (S2.1's `uv` row).

    S2.1's *Asserted by* cell names ``uv --version``, and that is what runs wherever
    `uv` is on the path. It is not on the path in the S7.3 runtime image — the
    Dockerfile copies `uv` into the *builder* stage, and the runtime stage takes only
    ``/app/.venv`` from it — and S9.1 runs `er doctor` inside that image, so the named
    mechanism is unavailable in the one environment this check matters most in. The
    fallback reads the ``uv = <version>`` marker `uv` itself writes into the
    virtualenv's ``pyvenv.cfg``: the version that resolved the lockfile this
    environment was built from, recorded by the same tool, travelling with the
    ``.venv`` the runtime stage copies. Neither branch can pass on a wrong `uv`.
    """
    executable = shutil.which("uv")
    if executable is None:
        return _venv_uv_version()
    completed = subprocess.run(
        [executable, "--version"], capture_output=True, text=True, check=False
    )
    if completed.returncode != 0:
        return f"{ERROR_PREFIX}uv --version exited {completed.returncode}"
    # `uv --version` prints `uv 0.11.3 (<commit> <date> <target>)`.
    parts = completed.stdout.split()
    return parts[1] if len(parts) > 1 else f"{ERROR_PREFIX}unreadable: {completed.stdout!r}"


def _venv_uv_version() -> str:
    """The ``uv = <version>`` line of the running interpreter's ``pyvenv.cfg``."""
    marker = Path(sys.prefix) / "pyvenv.cfg"
    try:
        text = marker.read_text(encoding="utf-8")
    except OSError as exc:
        return f"{ERROR_PREFIX}no uv on PATH and {marker} is unreadable: {exc}"
    for line in text.splitlines():
        key, separator, value = line.partition("=")
        if separator and key.strip() == "uv":
            return value.strip()
    return f"{ERROR_PREFIX}no uv on PATH and {marker} records no uv version"


def _dbt_adapter_version(context: DoctorContext) -> str:
    """dbt-duckdb's adapter version, read from `dbt debug` (S2.1's dbt-duckdb row).

    S2.1 assigns this row "adapter version + ``dbt debug --target mem``", and one
    invocation supplies both: `dbt debug` prints ``adapter version: <v>`` before it
    reaches its checks, and its exit status is the ``dbt debug`` half.

    ``--connection`` is not a narrowing of that check, it is what makes it runnable at
    all: without it `dbt debug` verifies that `git` is installed, which
    ``python:3.12-slim`` does not ship, so a plain `dbt debug` inside the S7.3 image
    would fail on a missing *linter* dependency rather than on anything about dbt.
    With it, the profile, the project and the warehouse connection are all still
    verified; only the `git` probe and the configuration summary are skipped.

    Spawned directly rather than through :func:`~er.dbt_runner.run_dbt`, which is the
    invoker for *stages*: it always passes ``--vars`` (which `dbt debug` does not
    take), writes a run log and raises :class:`~er.errors.StageFailure` on a non-zero
    status. A probe needs a verdict, not an exception.
    """
    # S4.0b: no Python DuckDB connection may span a dbt subprocess.
    context.close_lake()
    completed = subprocess.run(
        [
            "dbt",
            "debug",
            "--connection",
            "--project-dir",
            DBT_PROJECT_DIR,
            "--profiles-dir",
            DBT_PROFILES_DIR,
            "--target",
            "mem",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        return f"{ERROR_PREFIX}dbt debug exited {completed.returncode}"
    for line in completed.stdout.splitlines():
        _, separator, value = line.partition("adapter version:")
        if separator:
            return value.strip()
    return f"{ERROR_PREFIX}dbt debug printed no adapter version"


def _catalog_image(_: DoctorContext) -> str:
    """The catalog's major version, against the `postgres:16` image pin (S2.1).

    S2.1 gives the catalog image row a *behavioural* probe — ``server_version`` —
    rather than a digest comparison, because the digest is `docker/compose.yaml`'s to
    carry and `scripts/lint_spec.py`'s to check (S9.1). What a running process can
    prove is that the engine it is talking to is the major the pinned image is.
    """
    with catalog_connect() as connection:
        reported = server_version(connection)
    return f"postgres {reported.split('.')[0]}"


def _object_store_image(context: DoctorContext) -> str:
    """That the pinned object-store image is up and speaking S3 (S2.1).

    Deliberately weaker than the DATA_PATH round-trip, and deliberately independent of
    it: a listing that comes back as an S3 *error* — a missing bucket, say — still
    proves the store answered, which is all this row can assert about an image S2.1
    pins by digest. Keeping it independent is what leaves the round-trip row
    individually falsifiable (T-DOCTOR-1).
    """
    try:
        context.store().list_prefix(require_env("ER_LAKE_DATA_PATH"))
    except ClientError:
        # botocore raises `ClientError` — or one of the per-code subclasses its error
        # factory mints, such as `NoSuchBucket` — when the *server* answered with an
        # S3 error, and a connection error when nothing answered at all. Only the
        # second falsifies this row: an image that returns `NoSuchBucket` is up.
        return OK
    except Exception as exc:
        return f"{ERROR_PREFIX}{exc}"
    return OK


def _lake_snapshots(context: DoctorContext) -> str:
    """``SELECT * FROM lake.snapshots()`` succeeds (T-DOCTOR-1).

    The strongest single statement about the attachment: it needs the `ducklake`
    extension loaded, the catalog reachable and the metadata schema readable, and an
    extension that does not match the engine fails it at ATTACH rather than at query
    time (S2.1).
    """
    context.lake().execute(f"SELECT * FROM {LAKE_ALIAS}.snapshots()").fetchall()
    return OK


def _extensions_loaded(context: DoctorContext) -> str:
    """All three S2.1 extensions report ``loaded`` (T-DOCTOR-1).

    Returns the loaded names in :data:`~er.versions.EXTENSION_PINS` order, so the
    ``actual`` of a failed row names which extension is missing rather than a count.
    """
    registered = [pin.registered_name for pin in EXTENSION_PINS.values()]
    placeholders = ", ".join("?" for _ in registered)
    rows = (
        context.lake()
        .execute(
            "SELECT extension_name FROM duckdb_extensions() "
            f"WHERE loaded AND extension_name IN ({placeholders})",
            registered,
        )
        .fetchall()
    )
    loaded = {str(name) for (name,) in rows}
    return ",".join(name for name in registered if name in loaded)


def _dbt_target_database(_: DoctorContext) -> str:
    """The database dbt addresses lake relations under equals ``$ER_LAKE_ALIAS``.

    Read from the ``lake`` target's ``attach:`` alias, which is the name every
    ``lake.main.<relation>`` reference in `dbt/models/sources.yml` resolves through
    (S4.0b's dbt rendering of the attach). The alias is a literal in the profile —
    S7.2 makes it a constant, because a namespace is the pair (METADATA_SCHEMA,
    DATA_PATH) and "nothing else in the sequence varies" — so it is read as YAML
    rather than through Jinja, and an alias that had been templated would surface
    here as an unrendered value and fail loudly rather than pass.
    """
    profile = Path(DBT_PROFILES_DIR) / "profiles.yml"
    document = yaml.safe_load(profile.read_text(encoding="utf-8"))
    aliases = [str(entry["alias"]) for entry in document["er"]["outputs"]["lake"]["attach"]]
    environment = require_env("ER_LAKE_ALIAS")
    if aliases != [environment]:
        return f"dbt={aliases} env={environment}"
    return environment


def _data_path_round_trip(context: DoctorContext) -> str:
    """Write an object under ``$ER_LAKE_DATA_PATH``, read it back, delete it.

    S4.0b calls this "the backstop that catches a lake attached to the wrong place": a
    `DATA_PATH` rendered as the literal characters ``$ER_LAKE_DATA_PATH`` — the
    shell-form mistake that block forbids — attaches without error and fails far from
    its cause, and this is what turns that into a named check. The object is deleted
    on every path, so the doctor leaves the namespace exactly as it found it.
    """
    store = context.store()
    uri = require_env("ER_LAKE_DATA_PATH").rstrip("/") + "/" + _PROBE_OBJECT
    try:
        store.put_bytes(uri, _PROBE_PAYLOAD)
        if store.get_bytes(uri) != _PROBE_PAYLOAD:
            return f"{ERROR_PREFIX}{uri} read back different bytes"
    finally:
        _delete_probe_object(store, uri)
    return OK


def _delete_probe_object(store: ObjectStore, uri: str) -> None:
    """Remove the round-trip object, tolerating a write that never landed.

    :meth:`~er.lake.objectstore.ObjectStore.delete_prefix` is the deletion the S4.0b
    connection model leaves to boto3 — httpfs can neither enumerate nor delete a
    prefix — and its root guard is satisfied here because ``uri`` is a full object key
    under a namespace's DATA_PATH. A cleanup that fails must not overwrite the verdict
    of the round-trip it was cleaning up after.
    """
    try:
        store.delete_prefix(uri)
    except Exception:
        return


def _catalog_round_trip(_: DoctorContext) -> str:
    """A catalog connection that answers with ``server_version`` (T-DOCTOR-1).

    On its own connection, never the lake's: S2.1 pins `psycopg` precisely because the
    catalog must be reachable on a connection the DuckLake attachment does not own.
    """
    with catalog_connect() as connection:
        if not server_version(connection):
            return f"{ERROR_PREFIX}the catalog reported an empty server_version"
    return OK


def _no_splink_relations(context: DoctorContext) -> str:
    """Zero relations matching ``__splink__%`` in the lake (T-DOCTOR-1, M17).

    S4.0b hands Splink the same connection with ``output_schema='splink_scratch'`` so
    its intermediates land in the in-memory database. One that reached DuckLake would
    commit a snapshot per intermediate, which is a correctness problem long before it
    is a tidiness one.
    """
    database, _, schema = SCHEMA_QUALIFIER.partition(".")
    row = (
        context.lake().execute(_RELATIONS_SQL, [database, schema, f"{_SPLINK_PREFIX}%"]).fetchone()
    )
    return "0" if row is None else str(row[0])


# --- the table ------------------------------------------------------------------


def _pin_check(pin: Pin) -> DoctorCheck:
    """The check for one S2.1 row, or a refusal to build a table without it.

    The dispatch is on what S2.1's *Asserted by* cell names, ordered so that each case
    is unambiguous: the two image rows and the three extension rows have probes of
    their own, `uv` and the interpreter are neither distributions nor extensions,
    dbt-duckdb's cell names `dbt debug` as well as a version, and everything left is a
    distribution ``importlib.metadata`` can answer for.
    """
    if pin.component == "catalog-image":
        return DoctorCheck(pin.component, f"postgres {IMAGE_PINS['catalog'].tag}", _catalog_image)
    if pin.component == "object-store-image":
        return DoctorCheck(pin.component, OK, _object_store_image)
    if pin.component in EXTENSION_PINS:
        probe = _extension_probe(EXTENSION_PINS[pin.component])
        return DoctorCheck(pin.component, pin.version, probe)
    if pin.component == "python":
        return DoctorCheck(pin.component, pin.version, _python_version)
    if pin.component == "uv":
        return DoctorCheck(pin.component, pin.version, _uv_version)
    if pin.component == "dbt-duckdb":
        return DoctorCheck(pin.component, pin.version, _dbt_adapter_version)
    if pin.distribution is not None:
        return DoctorCheck(pin.component, pin.version, _distribution_probe(pin.distribution))
    raise MissingProbeError(pin.component)


def _runtime_checks() -> tuple[DoctorCheck, ...]:
    """The six T-DOCTOR-1 rows, in :data:`RUNTIME_CHECK_NAMES` order.

    Zipped against that tuple rather than each check naming itself, so the constant
    other modules read and the table this one builds cannot fall out of step.
    """
    probes: tuple[Callable[[DoctorContext], str], ...] = (
        _lake_snapshots,
        _extensions_loaded,
        _dbt_target_database,
        _data_path_round_trip,
        _catalog_round_trip,
        _no_splink_relations,
    )
    expected = (
        OK,
        ",".join(pin.registered_name for pin in EXTENSION_PINS.values()),
        LAKE_ALIAS,
        OK,
        OK,
        "0",
    )
    return tuple(
        DoctorCheck(name, value, probe)
        for name, value, probe in zip(RUNTIME_CHECK_NAMES, expected, probes, strict=True)
    )


def build_checks(pins: Mapping[str, Pin] = PINS) -> tuple[DoctorCheck, ...]:
    """The doctor's table: every ``asserted_by_doctor`` S2.1 row, then the six.

    ``pins`` is a parameter so the unit layer can build a table from a *mutated* S2.1
    and watch the corresponding line go red without touching the real one.

    Raises:
        MissingProbeError: a row names `er doctor` and has no probe here.
    """
    asserted = tuple(_pin_check(pin) for pin in pins.values() if pin.asserted_by_doctor)
    return (*asserted, *_runtime_checks())


#: The table `er doctor` runs, in print order: S2.1's rows, then T-DOCTOR-1's six.
CHECKS: Final[tuple[DoctorCheck, ...]] = build_checks()


def run_checks(
    checks: Sequence[DoctorCheck] | None = None,
    context: DoctorContext | None = None,
) -> tuple[CheckResult, ...]:
    """Run every check and report what each found, in table order.

    Every check runs even after one has failed, and a probe that raises becomes a
    failed row rather than a traceback: S4.0 gives the command a table and an exit
    code, and an operator told only about the first broken thing pays for a second run
    to learn about the second.

    Args:
        checks: the table to run; :data:`CHECKS` by default. Resolved at call time, so
            a test can substitute one.
        context: the shared resources; a fresh one by default. Its lake connection is
            closed on every exit path, including the failure path (S4.0b).
    """
    table = CHECKS if checks is None else tuple(checks)
    shared = DoctorContext() if context is None else context
    try:
        return tuple(_run_one(check, shared) for check in table)
    finally:
        shared.close_lake()


def _run_one(check: DoctorCheck, context: DoctorContext) -> CheckResult:
    """One row of the table, with a raising probe turned into a failed verdict."""
    try:
        actual = check.probe(context)
    except Exception as exc:
        actual = f"{ERROR_PREFIX}{type(exc).__name__}: {exc}"
    return CheckResult(
        name=check.name, expected=check.expected, actual=actual, ok=actual == check.expected
    )


def exit_code(results: Sequence[CheckResult]) -> int:
    """`0` when every check passed, `1` when any failed (S4.0, S2.1).

    ``1`` and never ``3``: a pin mismatch is a check failure under S4.0's exit-code
    table, and ``3`` is reserved there for the five named precondition failures.
    """
    return 0 if all(result.ok for result in results) else 1


def check_lines(results: Sequence[CheckResult]) -> Iterator[str]:
    """The human table S4.0 asks for: name, expected, actual, verdict, one per line.

    Column widths come from the longest value present rather than from constants, so a
    long ``error:`` string does not throw the rest of the table out of alignment.
    """
    name_width = max((len(result.name) for result in results), default=0)
    expected_width = max((len(result.expected) for result in results), default=0)
    for result in results:
        yield (
            f"{result.name:<{name_width}}  {result.expected:<{expected_width}}  "
            f"{result.actual}  {result.verdict}"
        )


def check_record(result: CheckResult) -> dict[str, object]:
    """One ``--json`` object: the same four columns, machine-readable (S4.0)."""
    return {
        "check": result.name,
        "expected": result.expected,
        "actual": result.actual,
        "verdict": result.verdict,
    }
