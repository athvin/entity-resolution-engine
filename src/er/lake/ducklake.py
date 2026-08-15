"""The one DuckDB connection factory every stage opens (DesignDoc.md S4.0b, S7.2).

S4.0b fixes the statement sequence — extension directory, autoinstall/autoload off,
the three extensions, the pinned ``threads``/``memory_limit``, the ``er_s3`` secret,
the ``ATTACH`` of the lake — and S7.2 makes this module the single place that emits
it, "verbatim and in that order", so the SQL side and the dbt side of the contract
cannot drift apart.

**Why every value is substituted in Python.** S4.0b verifies this against the pinned
``duckdb==1.5.5`` Python API rather than asserting it: ``getenv()`` is a DuckDB *CLI*
function and does not exist on a Python connection, and ``ATTACH`` accepts a bare
string literal only — not a function call, not a ``||`` concatenation, not a bound
``?`` parameter. So there is one substitution mechanism, Python string building, and
it goes through :func:`sql_literal`.

The failure that discipline prevents is silent. In DuckDB SQL a bare ``$NAME`` is a
prepared-statement parameter marker and ``'$NAME'`` is the literal characters
``$NAME``; a block that mixed those forms would execute without error while creating
a secret whose ``KEY_ID`` is the string ``$ER_S3_ACCESS_KEY_ID`` and attaching a lake
whose ``DATA_PATH`` is the string ``$ER_LAKE_DATA_PATH`` — both failing far from
their cause. Rendering therefore happens **before** the connection is opened, so a
missing variable raises ``ERR_ENV_MISSING: <name>`` (exit ``2``) with zero statements
executed rather than half an attach sequence.

Two consequences of S4.0b that callers must not undo:

* The primary database is ``:memory:`` and DuckLake is **never** the default catalog.
  Every lake reference is ``lake.main.<relation>``.
* Splink's intermediates land in :data:`SPLINK_OUTPUT_SCHEMA`, which lives in that
  in-memory database, so no ``__splink__`` relation is ever committed to the lake.

Environment reads go through :mod:`er.lake.env`, which owns the absent-or-empty rule
and the spelling of its failure; this module adds the SQL escaping on top of it.
"""

from __future__ import annotations

import os
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from typing import Final

import duckdb

from er.lake.env import require_bool_env, require_env, require_int_env

__all__ = [
    "EXTENSION_DIRECTORY_DEFAULT",
    "EXTENSION_DIRECTORY_ENV",
    "LAKE_ALIAS",
    "SPLINK_OUTPUT_SCHEMA",
    "SnapshotRange",
    "attach_statements",
    "connect",
    "current_snapshot",
    "detach",
    "ducklake_uri",
    "env_literal",
    "snapshot_range",
    "sql_literal",
]

# The alias is a constant, not a rendered value. S7.2 (M22): a namespace is exactly
# the pair (METADATA_SCHEMA, DATA_PATH) and "nothing else in the sequence varies", so
# `$ER_LAKE_ALIAS` names this constant rather than parameterising it — and an alias is
# an identifier, which is the one position a quoted literal could not fill anyway.
LAKE_ALIAS: Final[str] = "lake"

# S4.0b: Splink receives this connection as `DuckDBAPI(connection=…,
# output_schema='splink_scratch')`, which is what keeps `__splink__` intermediates in
# the in-memory database instead of committing a snapshot per intermediate to the lake.
SPLINK_OUTPUT_SCHEMA: Final[str] = "splink_scratch"

# Where S7.3 bakes the three extensions at image build time.
EXTENSION_DIRECTORY_DEFAULT: Final[str] = "/opt/duckdb_extensions"

# The escape hatch for a developer running outside the image, who has no
# `/opt/duckdb_extensions`. Autoinstall and autoload stay disabled either way: the
# point of the baked directory is that no container ever downloads an extension at
# runtime, and a fallback that quietly re-enabled downloading would defeat it.
EXTENSION_DIRECTORY_ENV: Final[str] = "ER_DUCKDB_EXTENSION_DIR"

# `INSTALL` is a no-op against the baked directory; both statements are still emitted
# because S4.0b's block emits both and this module renders that block verbatim.
_EXTENSIONS: Final[tuple[str, ...]] = ("ducklake", "postgres", "httpfs")

# DuckLake exposes the snapshot log as a table function on the attached catalog.
_SNAPSHOTS: Final[str] = f"{LAKE_ALIAS}.snapshots()"


def sql_literal(value: str) -> str:
    """Render ``value`` as a single-quoted SQL string, doubling embedded quotes.

    The only escaping mechanism in this module: every string position in the S4.0b
    block — including the ``ATTACH`` URI, which cannot be a bound parameter — is
    built from it.
    """
    escaped = value.replace("'", "''")
    return f"'{escaped}'"


def env_literal(name: str) -> str:
    """Render ``$name`` as a SQL string literal.

    Raises :class:`~er.lake.env.MissingEnvError` when the variable is absent or
    empty, which is the point: an empty value renders as a perfectly valid empty
    literal and attaches a lake that authenticates as nobody.
    """
    return sql_literal(require_env(name))


def ducklake_uri(dsn: str) -> str:
    """The ``ducklake:postgres:<dsn>`` URI ``ATTACH`` takes.

    Returned unquoted; :func:`attach_statements` passes it through
    :func:`sql_literal` like every other string position. ``postgresql://`` DSNs are
    accepted verbatim by the postgres extension and need no translation (S4.0b).
    """
    return f"ducklake:postgres:{dsn}"


def _extension_directory() -> str:
    """The directory extensions are loaded from, with the S7.3 baked path as default."""
    override = os.environ.get(EXTENSION_DIRECTORY_ENV)
    if override is None or not override.strip():
        return EXTENSION_DIRECTORY_DEFAULT
    return override


def attach_statements() -> tuple[str, ...]:
    """The S4.0b statement block, rendered against the process environment.

    Returned as one statement per element and in the spec's order, because that
    order is normative: the extension directory must be set before an extension is
    loaded, the secret must exist before the lake it authenticates is attached, and
    ``threads``/``memory_limit`` must be pinned on *every* connection (S7.1: DuckDB
    reads neither cgroup, so it otherwise plans against host RAM and host cores).

    Every variable is read here, before the caller opens a connection, so an absent
    one fails with nothing executed.
    """
    threads = require_int_env("ER_DUCKDB_THREADS")
    use_ssl = require_bool_env("ER_S3_USE_SSL")
    secret = "\n".join(
        (
            "CREATE OR REPLACE SECRET er_s3 (",
            "    TYPE s3,",
            f"    KEY_ID     {env_literal('ER_S3_ACCESS_KEY_ID')},",
            f"    SECRET     {env_literal('ER_S3_SECRET_ACCESS_KEY')},",
            # host:port with no scheme (S7.1); `objectstore.py` derives boto3's URL
            # from the same variable, so neither engine sees the other's spelling.
            f"    ENDPOINT   {env_literal('ER_S3_ENDPOINT')},",
            f"    URL_STYLE  {env_literal('ER_S3_URL_STYLE')},",
            f"    USE_SSL    {'true' if use_ssl else 'false'},",
            f"    REGION     {env_literal('ER_S3_REGION')}",
            ")",
        )
    )
    attach = "\n".join(
        (
            f"ATTACH IF NOT EXISTS {sql_literal(ducklake_uri(require_env('ER_CATALOG_DSN')))} "
            f"AS {LAKE_ALIAS} (",
            f"    DATA_PATH       {env_literal('ER_LAKE_DATA_PATH')},",
            f"    METADATA_SCHEMA {env_literal('ER_LAKE_METADATA_SCHEMA')}",
            ")",
        )
    )
    return (
        f"SET extension_directory = {sql_literal(_extension_directory())}",
        "SET autoinstall_known_extensions = false",
        "SET autoload_known_extensions   = false",
        *(f"{verb} {extension}" for extension in _EXTENSIONS for verb in ("INSTALL", "LOAD")),
        f"SET threads       = {threads}",
        f"SET memory_limit  = {env_literal('ER_DUCKDB_MEMORY_LIMIT')}",
        secret,
        attach,
    )


def detach(connection: duckdb.DuckDBPyConnection) -> bool:
    """DETACH the lake if this connection holds it; report whether it did.

    Checked rather than issued blind so the teardown path is idempotent: `connect`
    calls it in a ``finally`` that also runs when the ``ATTACH`` itself failed.
    """
    row = connection.execute(
        "SELECT count(*) FROM duckdb_databases() WHERE database_name = ?", [LAKE_ALIAS]
    ).fetchone()
    if row is None or not row[0]:
        return False
    connection.execute(f"DETACH {LAKE_ALIAS}")
    return True


@contextmanager
def connect() -> Iterator[duckdb.DuckDBPyConnection]:
    """Open the S4.0b connection: ``:memory:`` primary, lake attached as ``lake``.

    The block is rendered first and executed second, so a missing or malformed
    variable raises before a connection exists. On exit the lake is detached and the
    connection is closed on every path, including the failure path — S4.0b forbids a
    Python DuckDB connection from spanning a dbt subprocess, so a leaked connection
    is a correctness problem and not just a resource one.
    """
    statements = attach_statements()
    connection = duckdb.connect(":memory:")
    try:
        # Created before the ATTACH so that "the scratch schema is in the primary
        # database" is a property of this statement rather than of DuckDB's
        # default-catalog rules, which is what M17 actually requires.
        connection.execute(f"CREATE SCHEMA IF NOT EXISTS {SPLINK_OUTPUT_SCHEMA}")
        for statement in statements:
            connection.execute(statement)
        yield connection
    finally:
        detach(connection)
        connection.close()


def current_snapshot(connection: duckdb.DuckDBPyConnection) -> int:
    """The lake's current snapshot version."""
    row = connection.execute(f"SELECT max(snapshot_id) FROM {_SNAPSHOTS}").fetchone()
    if row is None or row[0] is None:
        # Unreachable against an attached lake — DuckLake writes its initial snapshot
        # when the namespace is created — but a range starting at 0 is the honest
        # answer for a lake with no snapshot log at all.
        return 0
    return int(row[0])


@dataclass
class SnapshotRange:
    """The ``(snapshot_start, snapshot_end)`` pair `run_stages` persists (S5.2).

    A stage MAY commit many snapshots, so the **range** is the unit of time travel
    and rollback (S4 preamble); nothing here counts snapshots, and nothing may.
    """

    start: int
    end: int

    def __iter__(self) -> Iterator[int]:
        """Unpack as ``start, end`` — the order S5.2 names the two columns in."""
        yield self.start
        yield self.end


@contextmanager
def snapshot_range(connection: duckdb.DuckDBPyConnection) -> Iterator[SnapshotRange]:
    """Record the snapshot range a block of work produced.

    ``end`` is filled in on exit, including when the block raises: a failed stage
    still committed whatever it committed before it failed, and `run_stages` records
    that range so the rollback target is knowable.
    """
    start = current_snapshot(connection)
    span = SnapshotRange(start=start, end=start)
    try:
        yield span
    finally:
        span.end = current_snapshot(connection)
