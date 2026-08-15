"""The direct Postgres catalog connection (DesignDoc.md S4.0, S4.0b).

S2.1's ``psycopg`` row states why this is not a DuckDB connection: "the tenant
advisory lock (S4.0b) and catalog-schema teardown must run on a Postgres connection
the DuckLake attachment does not own, since the lock outlives every DuckDB
connection in the process". S4.0b makes that concrete — the lock is "taken on
``$ER_CATALOG_DSN`` for the lifetime of the process and released on exit including on
failure" — and S4.0b also forbids a Python DuckDB connection from spanning a dbt
subprocess, so a lock living on one would be dropped in the middle of a run.

``postgresql://`` DSNs are accepted verbatim; S4.0b says they "need **no**
translation", and psycopg reads exactly that form.

This module imports no DuckDB. That is asserted on the import graph rather than left
to review, because the failure it prevents — a lock quietly released mid-run — is
invisible until two writers have already corrupted a namespace.
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterator
from contextlib import contextmanager
from typing import Any, Final

import psycopg
from psycopg import Connection, sql

from er.errors import ErrorClass, PreconditionFailure
from er.lake.env import require_env

__all__ = [
    "LOCK_APPLICATION_PREFIX",
    "LOCK_HELD_MESSAGE",
    "UNKNOWN_RUN",
    "advisory_lock",
    "advisory_lock_key",
    "catalog_connect",
    "drop_metadata_schema",
    "lock_holder_run_id",
    "metadata_schema_exists",
    "read_data_path",
    "server_version",
    "tenant_lock",
    "try_advisory_lock",
]

CatalogConnection = Connection[tuple[Any, ...]]

# DuckLake records its settings as key/value rows in this table inside the metadata
# schema; `data_path` is the one this project reads, because S4.0 makes a `DATA_PATH`
# that disagrees with `$ER_LAKE_DATA_PATH` a hard exit-3 rather than a re-point.
_METADATA_TABLE: Final[str] = "ducklake_metadata"
_DATA_PATH_KEY: Final[str] = "data_path"

# `pg_advisory_lock` takes a signed bigint, so the digest is truncated to eight
# bytes and read as signed. blake2b rather than `hash()`: PYTHONHASHSEED randomises
# `hash()` per process, and two processes that derived different keys for one tenant
# would both believe they held the lock.
_KEY_BYTES: Final[int] = 8

#: The refusal S4.7 spells literally: "failure to acquire exits `3` with the literal
#: message ``writer lock held for tenant <t> by run <run_id>``". A format string
#: rather than an f-string at the raise site, so the wording has one definition and a
#: test can assert the rendered message against it.
LOCK_HELD_MESSAGE: Final[str] = "writer lock held for tenant {tenant} by run {run_id}"

#: How a writer advertises which run holds the lock: its `application_name` becomes
#: this prefix followed by the `run_id`. Postgres already ties that string to the
#: session, so a writer that dies takes its advertisement with it exactly as it takes
#: the lock -- which a row in a table naming the holder would not.
LOCK_APPLICATION_PREFIX: Final[str] = "er-writer:"

#: The `<run_id>` of the message when the holder did not advertise one, or released
#: between the refused acquire and the lookup. The refusal is still exit `3` and the
#: message still has S4.7's shape; only the name of the other writer is unavailable.
UNKNOWN_RUN: Final[str] = "unknown"

# `pg_locks` splits a 64-bit advisory key across two `oid` columns and records
# `objsubid = 1` for the single-bigint form of the lock (`= 2` is the 32/32 form,
# which this project never takes).
_ADVISORY_OBJSUBID: Final[int] = 1
_UINT32_MASK: Final[int] = 0xFFFFFFFF
_UINT64_MASK: Final[int] = 0xFFFFFFFFFFFFFFFF


@contextmanager
def catalog_connect(dsn: str | None = None) -> Iterator[CatalogConnection]:
    """Open a catalog connection, closing it on every exit path.

    ``dsn`` defaults to ``$ER_CATALOG_DSN``. ``autocommit`` is on: every statement
    this module issues is a single DDL or a session-scoped lock call, and leaving an
    implicit transaction open around an advisory lock would hold catalog locks for
    the life of the process for no reason.
    """
    resolved = dsn if dsn is not None else require_env("ER_CATALOG_DSN")
    connection: CatalogConnection = psycopg.connect(resolved, autocommit=True)
    try:
        yield connection
    finally:
        connection.close()


def server_version(connection: CatalogConnection) -> str:
    """The catalog's ``server_version``, as Postgres reports it.

    S2.1 pins the catalog image to ``postgres:16`` and `er doctor` asserts this
    (T-DOCTOR-1), so the value is a pin check, not a diagnostic.
    """
    row = connection.execute("SELECT current_setting('server_version')").fetchone()
    if row is None:
        raise PreconditionFailure("the catalog returned no row for server_version")
    return str(row[0])


def metadata_schema_exists(connection: CatalogConnection, schema: str) -> bool:
    """Whether ``schema`` exists in the catalog."""
    row = connection.execute(
        "SELECT EXISTS (SELECT 1 FROM information_schema.schemata WHERE schema_name = %s)",
        (schema,),
    ).fetchone()
    return bool(row is not None and row[0])


def drop_metadata_schema(connection: CatalogConnection, schema: str) -> bool:
    """Drop ``schema`` and everything in it; return whether it was there to drop.

    Idempotent, because both callers are teardown paths that may run after a failure
    that never got as far as creating the schema: ``er lake reset`` (S4.0) and the
    S8.1 harness ``finally``. The name is interpolated as an identifier, not as a
    string — a schema name reaching a ``DROP SCHEMA`` unescaped is the one place in
    this module where a bad value is unrecoverable.
    """
    if not metadata_schema_exists(connection, schema):
        return False
    connection.execute(sql.SQL("DROP SCHEMA {} CASCADE").format(sql.Identifier(schema)))
    return True


def read_data_path(connection: CatalogConnection, schema: str) -> str | None:
    """The ``DATA_PATH`` DuckLake recorded for ``schema``, or ``None``.

    ``None`` means the namespace has never been attached — there is no metadata
    schema, or no ``data_path`` row in it — which S4.0 treats as "first attach"
    rather than as a mismatch. A recorded value that differs from
    ``$ER_LAKE_DATA_PATH`` is the exit-3 immutability failure, and that comparison
    belongs to ``er init`` (ER-020), not here.
    """
    qualified = f"{schema}.{_METADATA_TABLE}"
    relation = connection.execute("SELECT to_regclass(%s)", (qualified,)).fetchone()
    if relation is None or relation[0] is None:
        return None
    row = connection.execute(
        sql.SQL('SELECT value FROM {}.{} WHERE "key" = %s').format(
            sql.Identifier(schema), sql.Identifier(_METADATA_TABLE)
        ),
        (_DATA_PATH_KEY,),
    ).fetchone()
    if row is None or row[0] is None:
        return None
    return str(row[0])


def advisory_lock_key(tenant: str) -> int:
    """The bigint ``tenant`` locks on. Deterministic across processes and runs."""
    digest = hashlib.blake2b(tenant.encode("utf-8"), digest_size=_KEY_BYTES).digest()
    return int.from_bytes(digest, "big", signed=True)


def lock_holder_run_id(connection: CatalogConnection, key: int) -> str:
    """The `run_id` of the session currently holding ``key``, or :data:`UNKNOWN_RUN`.

    Read from ``pg_locks`` joined to ``pg_stat_activity`` rather than from a table of
    this project's own: the holder is identified by a live session, and any record
    outside Postgres could outlive the session it names — which is the one thing a
    contended-lock message must never do.

    ``classid``/``objid`` are compared as ``bigint`` because they are declared ``oid``,
    which has no implicit comparison against a 64-bit parameter.
    """
    unsigned = key & _UINT64_MASK
    row = connection.execute(
        "SELECT activity.application_name "
        "FROM pg_locks AS lock "
        "JOIN pg_stat_activity AS activity ON activity.pid = lock.pid "
        "WHERE lock.locktype = 'advisory' AND lock.granted "
        "AND lock.classid::bigint = %s AND lock.objid::bigint = %s AND lock.objsubid = %s",
        (unsigned >> 32, unsigned & _UINT32_MASK, _ADVISORY_OBJSUBID),
    ).fetchone()
    if row is None or not row[0]:
        return UNKNOWN_RUN
    name = str(row[0])
    if not name.startswith(LOCK_APPLICATION_PREFIX):
        return UNKNOWN_RUN
    return name[len(LOCK_APPLICATION_PREFIX) :]


@contextmanager
def tenant_lock(
    tenant: str, *, run_id: str | None = None, dsn: str | None = None
) -> Iterator[CatalogConnection]:
    """Hold the S4.0b single-writer lock for ``tenant`` for the life of the block.

    The lock takes a connection of its own, opened here and closed in the ``finally``.
    That is required, not tidy: a session-scoped advisory lock is released the moment
    its session ends, so a lock taken on a pooled or per-statement connection would
    silently disappear while the writer kept running. Closing the connection is also
    what makes release unconditional — an explicit ``pg_advisory_unlock`` that never
    ran because the block raised would leave the namespace locked until the process
    died, and S4.7 requires a failed run to be followed by a successful acquire with
    no manual unlock step.

    Refusal is exit ``3`` (S4.0b: "Failure to acquire exits 3"), raised as a
    :class:`~er.errors.PreconditionFailure` classified ``lock_conflict`` so the
    operator sees a contended namespace rather than a generic precondition, and
    carrying S4.7's literal :data:`LOCK_HELD_MESSAGE`. It happens **before** anything
    is written: a caller that had already recorded its `runs` row would leave a ledger
    entry for a run that never ran (T-CONC-1).

    Args:
        tenant: the namespace this writer locks, per S1 and S4.0b — the name of the
            (metadata schema, data path) pair, never a row-level discriminator.
        run_id: advertised on the session so the next contender can name this writer.
            ``None`` for a caller that has no run to name, whose contender then reads
            :data:`UNKNOWN_RUN`.
        dsn: defaults to ``$ER_CATALOG_DSN``.
    """
    key = advisory_lock_key(tenant)
    with catalog_connect(dsn) as connection:
        if run_id is not None:
            # `set_config` rather than `SET application_name = …`: SET takes no bound
            # parameter, and a run id pasted into the statement text would be the one
            # unescaped value in this module.
            connection.execute(
                "SELECT set_config('application_name', %s, false)",
                (f"{LOCK_APPLICATION_PREFIX}{run_id}",),
            )
        row = connection.execute("SELECT pg_try_advisory_lock(%s)", (key,)).fetchone()
        if row is None or not row[0]:
            raise PreconditionFailure(
                LOCK_HELD_MESSAGE.format(tenant=tenant, run_id=lock_holder_run_id(connection, key)),
                error_class=ErrorClass.LOCK_CONFLICT,
            )
        try:
            yield connection
        finally:
            connection.execute("SELECT pg_advisory_unlock(%s)", (key,))


@contextmanager
def advisory_lock(tenant: str, dsn: str | None = None) -> Iterator[CatalogConnection]:
    """ER-015's name for :func:`tenant_lock`, kept for the callers that use it.

    It delegates rather than duplicating: two functions able to acquire the lock is
    two places the S4.0b critical section could be entered, and the second one would
    inevitably drift from the first.
    """
    with tenant_lock(tenant, dsn=dsn) as connection:
        yield connection


def try_advisory_lock(tenant: str, dsn: str | None = None) -> bool:
    """Whether ``tenant``'s lock is currently free.

    A probe: it acquires, releases and closes before returning, so a ``True`` says
    the lock was free an instant ago and nothing more. The writer path takes the
    lock with :func:`advisory_lock` and holds it; this exists for `er doctor` and
    for tests, which need to observe contention without becoming a second writer.
    """
    key = advisory_lock_key(tenant)
    with catalog_connect(dsn) as connection:
        row = connection.execute("SELECT pg_try_advisory_lock(%s)", (key,)).fetchone()
        acquired = bool(row is not None and row[0])
        if acquired:
            connection.execute("SELECT pg_advisory_unlock(%s)", (key,))
        return acquired
