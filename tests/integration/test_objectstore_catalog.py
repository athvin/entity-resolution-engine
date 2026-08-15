"""The S3 client and the catalog connection, against the real Compose substrate.

This ticket runs before the S8.1 session fixture exists (ER-018), so the module
provisions and reclaims its own namespace: an ``er_test_<ulid>`` catalog schema and
an ``s3://lake/test/<ulid>/`` prefix, both reclaimed in the fixture's ``finally`` and
then *verified* reclaimed from a second connection and a second client. That
verification is the point: `delete_prefix` and `drop_metadata_schema` are the two
operations the S8.1 harness teardown is built from, and a teardown that quietly left
objects behind would look identical to one that worked.

No lake is attached here. ER-016 owns `ducklake.py`, and the two modules under test
are required to be reachable without DuckDB at all — asserted below on the import
graph rather than by reading the imports.
"""

from __future__ import annotations

import subprocess
import sys
from collections.abc import Iterator
from dataclasses import dataclass

import pytest
from psycopg import sql
from ulid import ULID

from er.lake.catalog import (
    advisory_lock,
    advisory_lock_key,
    catalog_connect,
    drop_metadata_schema,
    metadata_schema_exists,
    read_data_path,
    server_version,
    try_advisory_lock,
)
from er.lake.env import InvalidEnvError, MissingEnvError, require_int_env
from er.lake.objectstore import ObjectStore, PrefixGuardError

# S2.1 pins the catalog image to `postgres:16`; `current_setting('server_version')`
# reports something like "16.10 (Debian ...)", so the pin is the leading component.
EXPECTED_CATALOG_MAJOR = "16"

# One page of `ListObjectsV2` is 1000 keys. 1005 is the smallest count that proves
# the second page is fetched while staying cheap enough for the PR path.
PAGINATION_OBJECTS = 1005

# Imported in a subprocess: this test process has already imported `duckdb` (the
# substrate smoke test does), so `sys.modules` in-process proves nothing.
IMPORT_PROBE = """
import sys

import er.lake.catalog
import er.lake.objectstore

leaked = sorted(name for name in sys.modules if name == "duckdb" or name.startswith("duckdb."))
print(",".join(leaked))
sys.exit(1 if leaked else 0)
"""


class Boom(Exception):
    """Raised inside an `advisory_lock` block to prove release on the failure path."""


@dataclass(frozen=True)
class Namespace:
    """A throwaway namespace: the S8.1 pair of (metadata schema, data path)."""

    schema: str
    prefix: str
    store: ObjectStore


@pytest.fixture(scope="module")
def namespace() -> Iterator[Namespace]:
    ns = str(ULID()).lower()
    handle = Namespace(
        schema=f"er_test_{ns}",
        prefix=f"s3://lake/test/{ns}/",
        store=ObjectStore.from_env(),
    )
    try:
        yield handle
    finally:
        handle.store.delete_prefix(handle.prefix)
        with catalog_connect() as connection:
            drop_metadata_schema(connection, handle.schema)

        # Second connection, second client: reclamation is asserted by something
        # other than the objects that performed it.
        with catalog_connect() as verifier:
            assert not metadata_schema_exists(verifier, handle.schema), (
                f"{handle.schema} survived teardown"
            )
        left_behind = ObjectStore.from_env().list_prefix(handle.prefix)
        assert left_behind == [], f"{handle.prefix} still holds {len(left_behind)} objects"


def test_prefix_round_trip(namespace: Namespace) -> None:
    prefix = f"{namespace.prefix}round_trip/"
    uri = f"{prefix}payload.bin"
    payload = b"\x00entity-resolution\xff"

    namespace.store.put_bytes(uri, payload)

    assert namespace.store.get_bytes(uri) == payload
    assert namespace.store.list_prefix(prefix) == [uri]

    assert namespace.store.delete_prefix(prefix) == 1
    assert namespace.store.list_prefix(prefix) == []


def test_delete_prefix_paginates_beyond_1000(namespace: Namespace) -> None:
    prefix = f"{namespace.prefix}paginated/"
    expected = sorted(f"{prefix}{index:05d}.bin" for index in range(PAGINATION_OBJECTS))
    for uri in expected:
        namespace.store.put_bytes(uri, b"x")

    # Sorted rather than compared in listing order: S3 lists lexicographically, but
    # what this asserts is that no key was dropped at the page boundary.
    assert sorted(namespace.store.list_prefix(prefix)) == expected

    assert namespace.store.delete_prefix(prefix) == PAGINATION_OBJECTS
    assert namespace.store.list_prefix(prefix) == []


def test_delete_prefix_refuses_bucket_root(namespace: Namespace) -> None:
    canary = f"{namespace.prefix}canary/object.bin"
    namespace.store.put_bytes(canary, b"survive")

    for prefix in ("", "s3://lake", "s3://lake/"):
        with pytest.raises(PrefixGuardError):
            namespace.store.delete_prefix(prefix)

    assert namespace.store.list_prefix(f"{namespace.prefix}canary/") == [canary], (
        "a refused delete_prefix removed objects before raising"
    )
    namespace.store.delete_prefix(f"{namespace.prefix}canary/")


def test_catalog_server_version() -> None:
    with catalog_connect() as connection:
        version = server_version(connection)

    assert version.startswith(EXPECTED_CATALOG_MAJOR), (
        f"the catalog reports {version!r}, not the postgres:16 image S2.1 pins"
    )


def test_drop_metadata_schema_is_idempotent(namespace: Namespace) -> None:
    schema = namespace.schema
    with catalog_connect() as connection:
        assert metadata_schema_exists(connection, schema) is False
        assert read_data_path(connection, schema) is None

        connection.execute(sql.SQL("CREATE SCHEMA {}").format(sql.Identifier(schema)))
        assert metadata_schema_exists(connection, schema) is True
        # The schema exists but has never been attached, so there is no metadata
        # table yet -- S4.0's "first attach" case, not a DATA_PATH mismatch.
        assert read_data_path(connection, schema) is None

        connection.execute(
            sql.SQL('CREATE TABLE {}.ducklake_metadata ("key" TEXT, value TEXT)').format(
                sql.Identifier(schema)
            )
        )
        connection.execute(
            sql.SQL("INSERT INTO {}.ducklake_metadata VALUES (%s, %s)").format(
                sql.Identifier(schema)
            ),
            ("data_path", namespace.prefix),
        )
        assert read_data_path(connection, schema) == namespace.prefix

        assert drop_metadata_schema(connection, schema) is True
        assert drop_metadata_schema(connection, schema) is False
        assert metadata_schema_exists(connection, schema) is False


def test_advisory_lock_is_exclusive_per_tenant_and_released_on_error() -> None:
    assert advisory_lock_key("test") == advisory_lock_key("test")
    assert advisory_lock_key("test") != advisory_lock_key("other")

    with advisory_lock("test"):
        assert try_advisory_lock("test") is False
        assert try_advisory_lock("other") is True
    assert try_advisory_lock("test") is True

    with pytest.raises(Boom):
        with advisory_lock("test"):
            raise Boom
    assert try_advisory_lock("test") is True, "the lock survived the block that raised inside it"


def test_missing_env_raises_err_env_missing(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("ER_S3_ACCESS_KEY_ID", raising=False)
    with pytest.raises(MissingEnvError) as missing:
        ObjectStore.from_env()
    assert str(missing.value) == "ERR_ENV_MISSING: ER_S3_ACCESS_KEY_ID"
    assert missing.value.code == 2

    # Not merely a non-integer: the value S4.0b's `SET threads = {…}` position would
    # paste straight into a statement if the read were not validated.
    monkeypatch.setenv("ER_DUCKDB_THREADS", "2; DROP")
    with pytest.raises(InvalidEnvError) as invalid:
        require_int_env("ER_DUCKDB_THREADS")
    assert invalid.value.code == 2


def test_modules_do_not_import_duckdb() -> None:
    probe = subprocess.run(
        [sys.executable, "-c", IMPORT_PROBE],
        capture_output=True,
        text=True,
        check=False,
    )
    assert probe.returncode == 0, (
        f"importing the lake clients pulled in duckdb: {probe.stdout.strip()}\n{probe.stderr}"
    )
