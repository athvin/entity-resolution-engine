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
    "advisory_lock",
    "advisory_lock_key",
    "catalog_connect",
    "drop_metadata_schema",
    "metadata_schema_exists",
    "read_data_path",
    "server_version",
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


@contextmanager
def advisory_lock(tenant: str, dsn: str | None = None) -> Iterator[CatalogConnection]:
    """Hold the S4.0b single-writer lock for ``tenant`` for the life of the block.

    The lock takes a connection of its own, opened here and closed in the ``finally``.
    That is required, not tidy: a session-scoped advisory lock is released the moment
    its session ends, so a lock taken on a pooled or per-statement connection would
    silently disappear while the writer kept running. Closing the connection is also
    what makes release unconditional — an explicit ``pg_advisory_unlock`` that never
    ran because the block raised would leave the namespace locked until the process
    died.

    Refusal is exit ``3`` (S4.0b: "Failure to acquire exits 3"), raised as a
    :class:`~er.errors.PreconditionFailure` classified ``lock_conflict`` so the
    operator sees a contended namespace rather than a generic precondition.
    """
    key = advisory_lock_key(tenant)
    with catalog_connect(dsn) as connection:
        row = connection.execute("SELECT pg_try_advisory_lock(%s)", (key,)).fetchone()
        if row is None or not row[0]:
            raise PreconditionFailure(
                f"advisory lock for tenant {tenant!r} is held by another writer",
                error_class=ErrorClass.LOCK_CONFLICT,
            )
        try:
            yield connection
        finally:
            connection.execute("SELECT pg_advisory_unlock(%s)", (key,))


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
