"""The substrate itself, asserted from inside the `pipeline` container.

This is the first integration test the board produces, and it deliberately proves
only what S7.1 promises: a catalog that answers, an object store whose bucket exists
and round-trips through httpfs, an artifacts mount that reaches the host, and the
complete S6.1 environment. Everything above that -- the lake attach, `er init`, the
session-namespaced fixture of S8.1 -- belongs to later tickets, so this module
imports nothing from `src/er` and attaches no lake. It has to be able to pass at a
point where none of that exists yet, and it has to fail for exactly one reason: the
substrate is wrong.

No new dependency is introduced for it either (S2.1 is a closed set): the catalog
check goes through DuckDB's `postgres` extension and the object-store check through
`httpfs`, both baked into the image at build time (S7.3), which is why every
connection here is opened with autoinstall and autoload off.
"""

from __future__ import annotations

import os
import uuid
from pathlib import Path

import duckdb

# S4.0b's baked directory. Loading out of it with both autoload flags off is what
# makes these checks offline: a LOAD that succeeds proves the binary was on disk.
EXTENSION_DIRECTORY = "/opt/duckdb_extensions"
ARTIFACTS = Path("/app/artifacts")

# The complete set S6.1 says the CLI reads. Restated here rather than parsed from
# DesignDoc.md because the spec is not copied into the runtime image;
# tests/unit/test_compose_contract.py is what holds `x-er-env` to the spec's copy.
REQUIRED_ENV = (
    "ER_CONFIG",
    "ER_CATALOG_DSN",
    "ER_S3_ENDPOINT",
    "ER_S3_ACCESS_KEY_ID",
    "ER_S3_SECRET_ACCESS_KEY",
    "ER_S3_REGION",
    "ER_S3_URL_STYLE",
    "ER_S3_USE_SSL",
    "ER_LAKE_DATA_PATH",
    "ER_LAKE_ALIAS",
    "ER_LAKE_METADATA_SCHEMA",
    "ER_DUCKDB_THREADS",
    "ER_DUCKDB_MEMORY_LIMIT",
)


def require_env(name: str) -> str:
    value = os.environ.get(name, "")
    assert value, f"{name} is unset or empty inside the pipeline container (S6.1)"
    return value


def sql_literal(value: str) -> str:
    """A single-quoted SQL string with embedded quotes doubled.

    The same discipline S4.0b requires of the lake connection: the value is rendered
    by the host and reaches the engine as a literal, because a bare `$NAME` in DuckDB
    SQL is a parameter marker and `'$NAME'` is the characters themselves -- both of
    which execute happily while meaning something else entirely.
    """
    escaped = value.replace("'", "''")
    return f"'{escaped}'"


def connect(*extensions: str) -> duckdb.DuckDBPyConnection:
    """An in-memory connection with the named extensions loaded from the baked dir."""
    connection = duckdb.connect(
        ":memory:",
        config={
            "extension_directory": EXTENSION_DIRECTORY,
            "autoinstall_known_extensions": False,
            "autoload_known_extensions": False,
        },
    )
    for extension in extensions:
        connection.execute(f"LOAD {extension}")
    return connection


def test_catalog_round_trip() -> None:
    dsn = require_env("ER_CATALOG_DSN")
    connection = connect("postgres")
    try:
        # READ_ONLY: this test proves the catalog answers, and nothing more -- the
        # relations in there are owned by `er init` (ER-020), not by a smoke check.
        connection.execute(f"ATTACH {sql_literal(dsn)} AS catalog_db (TYPE postgres, READ_ONLY)")
        inner = "SELECT current_setting('server_version') AS server_version"
        row = connection.execute(
            f"SELECT server_version FROM postgres_query('catalog_db', {sql_literal(inner)})"
        ).fetchone()
    finally:
        connection.close()

    assert row is not None, "the catalog returned no row for server_version"
    server_version = str(row[0])
    assert server_version and server_version[0].isdigit(), (
        f"the catalog reported {server_version!r} rather than a Postgres version"
    )


def test_objectstore_round_trip() -> None:
    use_ssl = require_env("ER_S3_USE_SSL")
    assert use_ssl in ("true", "false"), f"ER_S3_USE_SSL={use_ssl!r} is not a SQL boolean"

    connection = connect("httpfs")
    try:
        # USE_SSL is the one option emitted unquoted; every other value is a string
        # literal, exactly as S4.0b renders the pipeline's own secret.
        connection.execute(
            "CREATE OR REPLACE SECRET er_s3_substrate ("
            "  TYPE s3,"
            f"  KEY_ID     {sql_literal(require_env('ER_S3_ACCESS_KEY_ID'))},"
            f"  SECRET     {sql_literal(require_env('ER_S3_SECRET_ACCESS_KEY'))},"
            f"  ENDPOINT   {sql_literal(require_env('ER_S3_ENDPOINT'))},"
            f"  URL_STYLE  {sql_literal(require_env('ER_S3_URL_STYLE'))},"
            f"  USE_SSL    {use_ssl},"
            f"  REGION     {sql_literal(require_env('ER_S3_REGION'))}"
            ")"
        )

        # A fresh prefix per run: the bucket survives until `down -v`, and a probe
        # that overwrote a fixed key would pass against a stale object.
        prefix = f"s3://lake/smoke/{uuid.uuid4().hex}"
        target = f"{prefix}/probe.parquet"

        connection.execute(f"COPY (SELECT 42 AS probe) TO {sql_literal(target)} (FORMAT parquet)")
        row = connection.execute(
            f"SELECT probe FROM read_parquet({sql_literal(target)})"
        ).fetchone()
        listed = connection.execute(
            f"SELECT file FROM glob({sql_literal(prefix + '/*')})"
        ).fetchall()
    finally:
        connection.close()

    assert row == (42,), f"the object store returned {row!r} for what was written"
    assert [str(entry[0]) for entry in listed] == [target], (
        f"listing {prefix} through httpfs returned {listed!r}"
    )


def test_artifacts_mount_is_writable() -> None:
    assert ARTIFACTS.is_dir(), f"{ARTIFACTS} is not a directory inside the container"

    probe = ARTIFACTS / f"substrate_probe_{uuid.uuid4().hex}.txt"
    try:
        probe.write_text("ok", encoding="utf-8")
        assert probe.read_text(encoding="utf-8") == "ok"
    finally:
        probe.unlink(missing_ok=True)


def test_required_env_is_complete() -> None:
    missing = [name for name in REQUIRED_ENV if not os.environ.get(name)]
    assert not missing, f"the pipeline container is missing {missing} (S6.1)"

    endpoint = require_env("ER_S3_ENDPOINT")
    assert "://" not in endpoint, (
        f"ER_S3_ENDPOINT={endpoint!r} carries a scheme; DuckDB's httpfs rejects a URL"
    )

    data_path = require_env("ER_LAKE_DATA_PATH")
    assert data_path.startswith("s3://") and data_path.endswith("/"), (
        f"ER_LAKE_DATA_PATH={data_path!r} is not an s3:// prefix (S6.1 V14)"
    )
