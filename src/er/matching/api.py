"""The one Splink-facing connection construction, and the guard that proves it (S4.0b).

M17 is that Splink emits unqualified ``CREATE TABLE`` statements for every
intermediate it materializes. Against a connection whose default catalog is DuckLake
each of those becomes a Postgres catalog write, a Parquet object and **one snapshot
per statement** — which does not merely litter the namespace, it corrupts the
snapshot history T-SNAP-1 time-travels through. S4.0b's answer is a connection whose
primary database is ``:memory:`` with the lake attached as an alias, handed to Splink
as ``DuckDBAPI(connection=<conn>, output_schema='splink_scratch')``.

:func:`splink_api` is that construction and is meant to be the only one in the
repository: the S4.3.4 two-pass scorer builds two `Linker`s, ``--mode full`` builds a
third, and the S4.5 label-propagation loop is bound by the same rule, so a second
construction site is a second chance to omit the output schema.

The easy thing to miss — and the reason :func:`assert_no_splink_relations_in_lake`
exists as a callable rather than as a comment — is that ``output_schema`` alone is
**not** what keeps the intermediates out of the lake. Splink's ``SET schema`` names a
schema, not a catalog; if the lake were the default catalog, `splink_scratch` would be
created *in the lake* and every intermediate with it. The guarantee comes from the
primary database being ``:memory:``, so it is a property of the connection and has to
be assertable at runtime. It is exported here for `er doctor` (S8.3 T-DOCTOR-1's sixth
runtime assertion) and for the T-INV-1 finalizer, which both make the same claim.

The two determinism-relevant settings are pinned here as well as in
:func:`er.lake.ducklake.connect`. Not redundantly: this function accepts *a*
connection, and S4.3.4 unions two passes and persists the result, so a Splink-facing
connection that planned with a different thread count or reordered its output would
make two runs of one corpus disagree for a reason no test could attribute to it.
"""

from __future__ import annotations

from typing import Final

import duckdb
from splink import DuckDBAPI

from er.errors import StageFailure
from er.lake.ducklake import LAKE_ALIAS, SPLINK_OUTPUT_SCHEMA
from er.lake.env import require_int_env

__all__ = [
    "SPLINK_RELATION_PREFIX",
    "SPLINK_SCRATCH_SCHEMA",
    "assert_no_splink_relations_in_lake",
    "leaked_splink_relations",
    "splink_api",
]

#: The schema Splink materializes into, under this ticket's name for it. It is
#: :data:`er.lake.ducklake.SPLINK_OUTPUT_SCHEMA` and not a second literal: the
#: connection factory creates the schema and this module hands its name to Splink,
#: and two spellings of one schema name would be two schemas the day one of them
#: changed.
SPLINK_SCRATCH_SCHEMA: Final[str] = SPLINK_OUTPUT_SCHEMA

#: How Splink names everything it materializes. The `LIKE` pattern built from it is
#: the same one T-DOCTOR-1 and T-INV-1 assert on, so it is declared once.
SPLINK_RELATION_PREFIX: Final[str] = "__splink__"

_SPLINK_RELATION_PATTERN: Final[str] = f"{SPLINK_RELATION_PREFIX}%"

# S8.3 says "relations", and a leak is as real through a view as through a table:
# `duckdb_tables()` does not report views, so both catalogs are queried.
_LEAKED_RELATIONS_SQL: Final[str] = """
SELECT table_name FROM duckdb_tables()
 WHERE database_name = ? AND table_name LIKE ?
UNION ALL
SELECT view_name FROM duckdb_views()
 WHERE database_name = ? AND view_name LIKE ?
"""

# DuckDB reads neither cgroup CPU nor cgroup memory (S7.1), so the thread count is
# always explicit. Splink's plans are what this one is about: thread count changes
# the parallel plan, and a different plan produces a different output row order.
_THREADS_ENV: Final[str] = "ER_DUCKDB_THREADS"

# DuckDB may reorder rows for throughput unless told not to. AC5's "the same
# prediction twice yields identical row order" is exactly this setting, and it is
# pinned on the connection Splink receives rather than assumed from the default,
# which a caller is free to have changed.
_PRESERVE_INSERTION_ORDER: Final[str] = "SET preserve_insertion_order = true"


def _primary_database(connection: duckdb.DuckDBPyConnection) -> str:
    """The connection's default catalog, refusing the one S4.0b forbids.

    S4.0b's first bullet — "no stage ever makes DuckLake the default catalog" — is
    the whole basis of the isolation, and this is the point at which violating it
    stops being recoverable: Splink is about to be handed the connection, and its
    unqualified ``CREATE TABLE``s would land wherever this answers.
    """
    row = connection.execute("SELECT current_database()").fetchone()
    if row is None or not row[0]:
        raise StageFailure("the connection reports no current database")
    database = str(row[0])
    if database == LAKE_ALIAS:
        raise StageFailure(
            f"the Splink connection's default catalog is {LAKE_ALIAS!r}; S4.0b requires the "
            "primary database to be :memory: with the lake attached as an alias, because "
            f"{SPLINK_SCRATCH_SCHEMA!r} would otherwise be created in the lake"
        )
    return database


def splink_api(connection: duckdb.DuckDBPyConnection) -> DuckDBAPI:
    """The S4.0b Splink handle for ``connection``: scratch in memory, lake by alias.

    Args:
        connection: an open connection from :func:`er.lake.ducklake.connect` — a
            ``:memory:`` primary database with DuckLake attached as ``lake``.

    Returns:
        A ``DuckDBAPI`` over that same connection whose ``output_schema`` is
        :data:`SPLINK_SCRATCH_SCHEMA`. Splink's own constructor then issues
        ``SET schema``, so the connection's search path points at the scratch schema
        afterwards; every lake reference in this codebase is written ``lake.main.…``
        precisely so that repointing changes nothing.

    Raises:
        StageFailure: the connection's default catalog is the lake, which would put
            the scratch schema — and every intermediate in it — inside DuckLake.
        er.lake.env.MissingEnvError: ``ER_DUCKDB_THREADS`` is absent or empty.
        er.lake.env.InvalidEnvError: ``ER_DUCKDB_THREADS`` is not an integer.
    """
    database = _primary_database(connection)
    # Read before anything is executed, so a bad envelope leaves the connection's
    # settings as it found them rather than half-pinned.
    threads = require_int_env(_THREADS_ENV)
    connection.execute(f"SET threads = {threads}")
    connection.execute(_PRESERVE_INSERTION_ORDER)
    # Qualified by the primary database rather than left to the default catalog:
    # `DuckDBAPI` would create it unqualified, which is correct only for as long as
    # the guard above holds, and this ticket's guarantee should not depend on that
    # ordering.
    connection.execute(f'CREATE SCHEMA IF NOT EXISTS "{database}".{SPLINK_SCRATCH_SCHEMA}')
    return DuckDBAPI(connection=connection, output_schema=SPLINK_SCRATCH_SCHEMA)


def leaked_splink_relations(connection: duckdb.DuckDBPyConnection) -> tuple[str, ...]:
    """Every ``__splink__`` relation currently in the lake, sorted.

    Empty is the invariant; the names are returned rather than a count because the
    operator's next question after "something leaked" is always "what".
    """
    rows = connection.execute(
        _LEAKED_RELATIONS_SQL,
        [LAKE_ALIAS, _SPLINK_RELATION_PATTERN, LAKE_ALIAS, _SPLINK_RELATION_PATTERN],
    ).fetchall()
    return tuple(sorted(str(name) for (name,) in rows))


def assert_no_splink_relations_in_lake(connection: duckdb.DuckDBPyConnection) -> None:
    """Assert M17: no Splink intermediate has reached the lake.

    The executable form of the guarantee S4.0b makes and T-DOCTOR-1 and T-INV-1 both
    assert. It is a `StageFailure` (S4.0 exit ``1``) rather than a bare `AssertionError`
    because `er doctor` and the T-INV-1 finalizer are the callers, and a leak means the
    snapshot history is already wrong — every snapshot committed by a stray
    ``CREATE TABLE`` sits between the range endpoints `run_stages` recorded.

    Args:
        connection: any connection with the lake attached; the check reads the
            catalog, not the data, so it costs nothing to call after every stage.

    Raises:
        StageFailure: one or more ``__splink__`` relations exist in the lake.
    """
    leaked = leaked_splink_relations(connection)
    if leaked:
        raise StageFailure(
            f"{len(leaked)} Splink relation(s) reached {LAKE_ALIAS} (S4.0b, M17): "
            f"{', '.join(leaked)}. Splink must be constructed through splink_api(), whose "
            f"connection has a :memory: primary database and output_schema="
            f"{SPLINK_SCRATCH_SCHEMA!r}"
        )
