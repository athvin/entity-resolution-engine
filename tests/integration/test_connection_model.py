"""The S4.0b connection model, against the real Compose substrate.

What this file is really guarding is a class of failure that does **not** raise. A
block that emitted a shell-style `$ER_LAKE_DATA_PATH` would attach a lake whose data
path is the literal string `$ER_LAKE_DATA_PATH`; one that emitted `getenv(...)` in a
secret would fail loudly, but one that emitted `'$ER_S3_ACCESS_KEY_ID'` would create a
secret that authenticates as nobody. So the rendered text is asserted directly, not
only its effect, and the effect is asserted against a really-attached lake.

The ER-018 session harness does not exist yet, so this module provisions and reclaims
its own namespace -- an `er_test_<ulid>` metadata schema and an
`s3://lake/test/<ulid>/` data path, the S7.2 pair -- through `catalog.py` and
`objectstore.py` in a `finally`.

No test here asserts a snapshot **count**: the S4 preamble makes the snapshot *range*
the unit of time travel, and a stage may legitimately commit many snapshots or none.
"""

from __future__ import annotations

import re
import shutil
from collections.abc import Iterator, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import duckdb
import pytest
from ulid import ULID

from er.lake.catalog import catalog_connect, drop_metadata_schema
from er.lake.ducklake import (
    EXTENSION_DIRECTORY_DEFAULT,
    EXTENSION_DIRECTORY_ENV,
    LAKE_ALIAS,
    SPLINK_OUTPUT_SCHEMA,
    attach_statements,
    connect,
    current_snapshot,
    snapshot_range,
    sql_literal,
)
from er.lake.env import InvalidEnvError, MissingEnvError
from er.lake.objectstore import ObjectStore

# S4.0b's block, one entry per statement and in the spec's order. Prefixes rather
# than whole statements: the substituted positions are what the *other* tests are
# about, and pinning them here would make this test fail for the wrong reason.
EXPECTED_ORDER = (
    "SET extension_directory =",
    "SET autoinstall_known_extensions = false",
    "SET autoload_known_extensions",
    "INSTALL ducklake",
    "LOAD ducklake",
    "INSTALL postgres",
    "LOAD postgres",
    "INSTALL httpfs",
    "LOAD httpfs",
    "SET threads",
    "SET memory_limit",
    "CREATE OR REPLACE SECRET er_s3 (",
    "ATTACH IF NOT EXISTS ",
)

# `getenv(` does not exist on a Python connection; `||` and `?` are both parser
# errors inside `ATTACH`. Each of the three would fail loudly -- they are checked
# because a future edit reaching for one is the readable symptom of the mechanism
# drifting away from "one Python-side substitution".
FORBIDDEN_TOKENS = ("getenv(", "||", "?")

# The silent one. A bare `$NAME` is a prepared-statement parameter marker and
# `'$NAME'` is the literal characters `$NAME`; neither raises, and both attach the
# wrong lake.
SHELL_SUBSTITUTION = re.compile(r"\$[A-Za-z_{]")

# S2.1: `INSTALL postgres` is the install name, but `duckdb_extensions()` registers
# it as `postgres_scanner`. Asserting the install name would silently find no row.
LOADED_EXTENSIONS = ("ducklake", "postgres_scanner", "httpfs")

# Splink names its intermediates `__splink__*`; the point of S4.0b's output schema is
# that none of them can reach the lake (M17).
SCRATCH_TABLE = "__splink__df_er_016"

# Created in the lake by the time-travel and reconnect tests and dropped again in
# their `finally`, so the "no lake tables" assertion of the scratch test does not
# depend on the order the file's tests run in.
SPAN_TABLE = "er_016_spans"
RECONNECT_TABLE = "er_016_reconnect"


@dataclass(frozen=True)
class Namespace:
    """The throwaway (metadata schema, data path) pair this module attaches."""

    schema: str
    prefix: str


@pytest.fixture(scope="module")
def namespace() -> Iterator[Namespace]:
    identifier = str(ULID()).lower()
    handle = Namespace(schema=f"er_test_{identifier}", prefix=f"s3://lake/test/{identifier}/")
    # A module-scoped fixture cannot take the function-scoped `monkeypatch`, and the
    # namespace has to be in the environment before the first `connect()` renders it.
    with pytest.MonkeyPatch.context() as patch:
        patch.setenv("ER_LAKE_METADATA_SCHEMA", handle.schema)
        patch.setenv("ER_LAKE_DATA_PATH", handle.prefix)
        store = ObjectStore.from_env()
        try:
            yield handle
        finally:
            store.delete_prefix(handle.prefix)
            with catalog_connect() as connection:
                drop_metadata_schema(connection, handle.schema)


class RecordingConnection:
    """A stand-in for a DuckDB connection that executes nothing and remembers all.

    S4.0b requires the whole block to be rendered before any of it runs, and "zero
    statements executed" is not observable on a real connection: a half-applied
    sequence looks like a connection that simply failed.
    """

    def __init__(self) -> None:
        self.executed: list[str] = []

    def execute(self, statement: str, parameters: object = None) -> RecordingConnection:
        self.executed.append(statement)
        return self

    def fetchone(self) -> tuple[Any, ...] | None:
        return None

    def close(self) -> None:
        return None


def _scalar(
    connection: duckdb.DuckDBPyConnection, statement: str, parameters: Sequence[Any] | None = None
) -> Any:
    row = connection.execute(statement, parameters).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def _reported_memory_limit(limit: str) -> str:
    """What DuckDB itself reports after `SET memory_limit = <limit>`.

    DuckDB normalises the setting to a rounded human-readable string -- `'4GB'` comes
    back as `'3.7 GiB'` -- so comparing `current_setting` against the raw environment
    text would fail on a connection that applied it perfectly. A reference connection
    given the same SET is the exact comparison.
    """
    reference = duckdb.connect(":memory:")
    try:
        reference.execute(f"SET memory_limit = {sql_literal(limit)}")
        return str(_scalar(reference, "SELECT current_setting('memory_limit')"))
    finally:
        reference.close()


def test_statement_block_matches_spec_order(namespace: Namespace) -> None:
    statements = attach_statements()

    assert len(statements) == len(EXPECTED_ORDER), (
        f"the block has {len(statements)} statements, S4.0b's has {len(EXPECTED_ORDER)}:\n"
        + "\n".join(statements)
    )
    for statement, prefix in zip(statements, EXPECTED_ORDER, strict=True):
        assert statement.startswith(prefix), (
            f"expected a statement starting {prefix!r}, got:\n{statement}"
        )

    rendered = "\n".join(statements)
    for token in FORBIDDEN_TOKENS:
        assert token not in rendered, f"the rendered block contains {token!r}:\n{rendered}"
    assert not SHELL_SUBSTITUTION.search(rendered), (
        f"the rendered block carries a shell-style substitution:\n{rendered}"
    )
    assert f"AS {LAKE_ALIAS} (" in rendered
    assert sql_literal(namespace.prefix) in rendered
    assert sql_literal(namespace.schema) in rendered


def test_no_shell_substitution_and_quotes_are_doubled(
    namespace: Namespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    assert sql_literal("o'brien") == "'o''brien'"
    assert sql_literal("''") == "''''''"

    monkeypatch.setenv("ER_S3_SECRET_ACCESS_KEY", "o'brien")
    rendered = "\n".join(attach_statements())

    assert "SECRET     'o''brien'" in rendered
    assert not SHELL_SUBSTITUTION.search(rendered)
    # The connection still opens: a doubled quote is valid SQL, an undoubled one
    # would terminate the literal and leave `brien'` as syntax.
    with connect() as connection:
        assert _scalar(connection, "SELECT current_database()") != LAKE_ALIAS


def test_missing_env_raises_before_any_statement(
    namespace: Namespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    opened: list[RecordingConnection] = []

    def recording_connect(*args: object, **kwargs: object) -> RecordingConnection:
        opened.append(RecordingConnection())
        return opened[-1]

    monkeypatch.setattr(duckdb, "connect", recording_connect)

    monkeypatch.delenv("ER_LAKE_DATA_PATH")
    with pytest.raises(MissingEnvError) as missing:
        with connect():
            pass
    assert str(missing.value) == "ERR_ENV_MISSING: ER_LAKE_DATA_PATH"
    assert missing.value.code == 2
    assert opened == [], "a connection was opened before the environment was read"

    # Not merely a non-integer: the value `SET threads = {…}` would paste straight
    # into a statement if the read were not validated.
    monkeypatch.setenv("ER_LAKE_DATA_PATH", namespace.prefix)
    monkeypatch.setenv("ER_DUCKDB_THREADS", "2; DROP TABLE x")
    with pytest.raises(InvalidEnvError) as invalid:
        with connect():
            pass
    assert invalid.value.code == 2
    assert opened == [], "a connection was opened before the environment was validated"


def test_primary_database_is_memory_and_lake_is_attached(namespace: Namespace) -> None:
    with connect() as connection:
        primary = _scalar(connection, "SELECT current_database()")
        assert primary != LAKE_ALIAS, "DuckLake became the default catalog"
        assert not _scalar(
            connection,
            "SELECT path FROM duckdb_databases() WHERE database_name = current_database()",
        ), f"the primary database {primary!r} is not in memory"

        attached = _scalar(
            connection,
            "SELECT count(*) FROM duckdb_databases() WHERE database_name = ?",
            [LAKE_ALIAS],
        )
        assert attached == 1

        # Succeeding is the assertion; the row set deliberately is not, because no
        # test may assert how many snapshots a namespace holds.
        snapshots = connection.execute(f"SELECT * FROM {LAKE_ALIAS}.snapshots()")
        assert snapshots.description is not None
        assert "snapshot_id" in [column[0] for column in snapshots.description]


def test_extensions_loaded_with_autoinstall_disabled(namespace: Namespace) -> None:
    with connect() as connection:
        loaded = {
            row[0]
            for row in connection.execute(
                "SELECT extension_name FROM duckdb_extensions() WHERE loaded"
            ).fetchall()
        }
        assert set(LOADED_EXTENSIONS) <= loaded, f"loaded extensions: {sorted(loaded)}"

        for setting in ("autoinstall_known_extensions", "autoload_known_extensions"):
            assert _scalar(connection, f"SELECT current_setting('{setting}')") is False, (
                f"{setting} is enabled; the image bakes its extensions and no container "
                "may download one at runtime"
            )


def test_threads_and_memory_limit_are_pinned(
    namespace: Namespace, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Two different envelopes, because a single one could pass against a connection
    # that ignored the environment and happened to match DuckDB's own default.
    for threads, memory_limit in (("3", "512MB"), ("2", "1GB")):
        monkeypatch.setenv("ER_DUCKDB_THREADS", threads)
        monkeypatch.setenv("ER_DUCKDB_MEMORY_LIMIT", memory_limit)
        with connect() as connection:
            assert _scalar(connection, "SELECT current_setting('threads')") == int(threads)
            assert _scalar(connection, "SELECT current_setting('memory_limit')") == (
                _reported_memory_limit(memory_limit)
            )


def test_splink_scratch_stays_out_of_lake(namespace: Namespace) -> None:
    with connect() as connection:
        connection.execute(f"CREATE TABLE {SPLINK_OUTPUT_SCHEMA}.{SCRATCH_TABLE} AS SELECT 1 AS n")

        assert _scalar(
            connection,
            "SELECT database_name FROM duckdb_tables() WHERE table_name = ?",
            [SCRATCH_TABLE],
        ) == _scalar(connection, "SELECT current_database()")
        assert (
            _scalar(
                connection,
                "SELECT count(*) FROM duckdb_tables() WHERE database_name = ?",
                [LAKE_ALIAS],
            )
            == 0
        ), "a relation reached the lake from the in-memory database"

    # A second connection: the scratch schema is per-connection state, so nothing it
    # held may survive into the lake the next process attaches.
    with connect() as reopened:
        assert (
            _scalar(
                reopened,
                "SELECT count(*) FROM duckdb_tables() "
                "WHERE database_name = ? AND table_name LIKE '__splink__%'",
                [LAKE_ALIAS],
            )
            == 0
        )


def test_snapshot_range_supports_time_travel(namespace: Namespace) -> None:
    with connect() as connection:
        connection.execute(f"CREATE TABLE {LAKE_ALIAS}.main.{SPAN_TABLE} (n INTEGER)")
        try:
            with snapshot_range(connection) as span:
                connection.execute(f"INSERT INTO {LAKE_ALIAS}.main.{SPAN_TABLE} VALUES (1), (2)")

            assert span.end >= span.start
            assert span.end == current_snapshot(connection)
            start, end = span
            assert (start, end) == (span.start, span.end)

            visible = _scalar(
                connection,
                f"SELECT count(*) FROM {LAKE_ALIAS}.main.{SPAN_TABLE} AT (VERSION => {span.end})",
            )
            before = _scalar(
                connection,
                f"SELECT count(*) FROM {LAKE_ALIAS}.main.{SPAN_TABLE} AT (VERSION => {span.start})",
            )
            assert (visible, before) == (2, 0), (
                "the range does not bracket the write it was opened around"
            )
        finally:
            connection.execute(f"DROP TABLE IF EXISTS {LAKE_ALIAS}.main.{SPAN_TABLE}")


def test_extension_directory_fallback(
    namespace: Namespace, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.delenv(EXTENSION_DIRECTORY_ENV, raising=False)
    assert (
        attach_statements()[0]
        == f"SET extension_directory = {sql_literal(EXTENSION_DIRECTORY_DEFAULT)}"
    )

    # The non-image developer run: the same three extensions somewhere else.
    developer_directory = tmp_path / "duckdb_extensions"
    shutil.copytree(EXTENSION_DIRECTORY_DEFAULT, developer_directory)
    monkeypatch.setenv(EXTENSION_DIRECTORY_ENV, str(developer_directory))

    assert (
        attach_statements()[0]
        == f"SET extension_directory = {sql_literal(str(developer_directory))}"
    )
    with connect() as connection:
        assert _scalar(connection, "SELECT current_setting('extension_directory')") == str(
            developer_directory
        )
        assert (
            _scalar(
                connection,
                "SELECT count(*) FROM duckdb_databases() WHERE database_name = ?",
                [LAKE_ALIAS],
            )
            == 1
        )


def test_reconnect_after_detach(namespace: Namespace) -> None:
    with connect() as first:
        first.execute(f"CREATE TABLE {LAKE_ALIAS}.main.{RECONNECT_TABLE} (n INTEGER)")
        first.execute(f"INSERT INTO {LAKE_ALIAS}.main.{RECONNECT_TABLE} VALUES (7)")

    with pytest.raises(duckdb.Error):
        first.execute("SELECT 1")

    with connect() as second:
        try:
            # Reading back what the first connection committed is what proves the
            # second attached the *same* namespace, rather than merely attaching.
            assert _scalar(second, f"SELECT n FROM {LAKE_ALIAS}.main.{RECONNECT_TABLE}") == 7
        finally:
            second.execute(f"DROP TABLE IF EXISTS {LAKE_ALIAS}.main.{RECONNECT_TABLE}")
