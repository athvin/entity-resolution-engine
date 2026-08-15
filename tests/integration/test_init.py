"""`er init` and `er lake reset` against the Compose substrate (S4.0, S4.0b, S8.1).

M19 is that nothing anywhere invoked `ddl.py`, so every assertion here is made on
the *command* rather than on the function it calls: the CLI is spawned as a
subprocess, exactly as `catalog-init` spawns it (S7.1), and what is checked is the
exit code, the stdout contract and what survived in the catalog and the object
store afterwards. A test that called ``init_lake`` directly would leave the wiring
M19 is about — the verb, its exit code, its stdout — untested.

**Every destructive test runs in a namespace of its own.** `er lake reset` drops
the metadata schema and deletes the prefix; run against the S8.1 session namespace
it would pull the attachment `lake_conn` holds out from under every later test in
the session. A namespace is exactly the pair (``ER_LAKE_METADATA_SCHEMA``,
``ER_LAKE_DATA_PATH``) and nothing else in the attach sequence varies (S7.2), so a
second one costs two environment variables in the child's environment — and
overriding them in the child leaves the parent session's own namespace untouched.

S4.0's messages and S8.1's namespace spellings are written out as literals rather
than imported from the modules under test: a test that reused the implementation's
own constant could not catch a wrong constant, and AC2 is precisely a claim about
one exact string.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator, Mapping
from contextlib import contextmanager
from dataclasses import dataclass

import duckdb
import pytest
from ulid import ULID

from er.lake.catalog import (
    CatalogConnection,
    advisory_lock,
    drop_metadata_schema,
    metadata_schema_exists,
)
from er.lake.ducklake import LAKE_ALIAS, connect
from er.lake.model import DBT_OWNED, DDL_OWNED, SCHEMA_QUALIFIER
from er.lake.objectstore import ObjectStore

# S5.0's ownership table lists exactly this many `ddl.py`-owned relations, and S4.0
# makes `er init` print one line per relation. A literal, because a count derived
# from `DDL_OWNED` would agree with the registry no matter what the registry said.
DDL_OWNED_COUNT = 14

# `configs/test.yaml`'s `tenant:`, which Compose supplies to every service as
# `ER_CONFIG` (S7.1). `--confirm-tenant` is compared against this value, not against
# the metadata schema, so the destructive path cannot be reached by a typo (S4.0).
TENANT = "test"

# S4.0's immutability message, character for character including the remedy clause.
DATA_PATH_MISMATCH = (
    "lake DATA_PATH immutable: catalog={catalog} env={env}; "
    f"use 'er lake reset --confirm-tenant {TENANT}' to destroy and recreate this namespace"
)

# S8.1 step 2's two spellings, for the scratch namespaces below.
METADATA_SCHEMA = "er_test_{ns}"
DATA_PATH = "s3://lake/test/{ns}/"

# One row, so the namespace holds data that dropping the metadata schema destroys.
ENTITIES_ROW = "('E1', 'active', NULL, now(), now(), 'R1', 'R1')"

# What the prefix is seeded with. Written directly rather than left to the INSERT
# above: what DuckLake puts under `DATA_PATH` for a single small change is its own
# business, and these tests are about whether the prefix delete ran, not about
# DuckLake's file layout. The S8.1 teardown test seeds its prefix the same way.
PROBE_OBJECT = "probe.txt"
PROBE_BYTES = b"probe"


@dataclass(frozen=True)
class Scratch:
    """A namespace this test owns outright, reclaimed however the test ends."""

    ns: str
    schema: str
    data_path: str

    @property
    def env(self) -> dict[str, str]:
        """The two variables that make a child process attach this namespace."""
        return {
            "ER_LAKE_METADATA_SCHEMA": self.schema,
            "ER_LAKE_DATA_PATH": self.data_path,
        }


@pytest.fixture
def scratch(object_store: ObjectStore, catalog: CatalogConnection) -> Iterator[Scratch]:
    """A fresh, empty namespace, reclaimed under `try/finally` like S8.1's own.

    Reclaimed with the two primitives `er lake reset` uses rather than with the verb
    itself: a teardown built out of the command under test would report a green suite
    for a reset that silently did nothing.
    """
    ns = str(ULID()).lower()
    namespace = Scratch(
        ns=ns, schema=METADATA_SCHEMA.format(ns=ns), data_path=DATA_PATH.format(ns=ns)
    )
    try:
        yield namespace
    finally:
        drop_metadata_schema(catalog, namespace.schema)
        object_store.delete_prefix(namespace.data_path)


def run_er(*args: str, env: Mapping[str, str]) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script with ``env`` layered on this process's.

    The same entry point `catalog-init` runs (S7.1), so the exit code asserted here is
    the one Compose gates the whole stack on.
    """
    environment = dict(os.environ)
    environment.update(env)
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=environment, check=False
    )


@contextmanager
def attached(namespace: Scratch) -> Iterator[duckdb.DuckDBPyConnection]:
    """An S4.0b connection to ``namespace``, for asserting what the CLI left behind."""
    with pytest.MonkeyPatch.context() as patch:
        for name, value in namespace.env.items():
            patch.setenv(name, value)
        with connect() as connection:
            yield connection


def fill(namespace: Scratch, store: ObjectStore) -> list[str]:
    """Give ``namespace`` a row and an object, and report what is under its prefix.

    Both, because they are destroyed by different halves of `er lake reset`: the row
    by the ``DROP SCHEMA``, the object by the prefix delete.
    """
    with attached(namespace) as connection:
        connection.execute(f"INSERT INTO {SCHEMA_QUALIFIER}.entities VALUES {ENTITIES_ROW}")
    store.put_bytes(namespace.data_path + PROBE_OBJECT, PROBE_BYTES)
    keys = store.list_prefix(namespace.data_path)
    assert keys, "the namespace holds no objects, so 'deleted the prefix' asserts nothing"
    return keys


def relations(connection: duckdb.DuckDBPyConnection) -> set[str]:
    rows = connection.execute(
        "SELECT table_name FROM duckdb_tables() WHERE database_name = ?", [LAKE_ALIAS]
    ).fetchall()
    return {str(name) for (name,) in rows}


def actions(stdout: str) -> list[tuple[str, str]]:
    """`er init`'s human stdout as ``(relation, action)`` pairs, in emission order."""
    pairs: list[tuple[str, str]] = []
    for line in stdout.splitlines():
        relation, _, action = line.partition(" ")
        pairs.append((relation, action))
    return pairs


def test_init_creates_then_reports_exists(scratch: Scratch) -> None:
    """AC1: fourteen `created` lines, then fourteen `exists` lines and nothing done."""
    first = run_er("init", env=scratch.env)

    assert first.returncode == 0, first.stderr
    assert actions(first.stdout) == [(relation, "created") for relation in DDL_OWNED]
    assert len(actions(first.stdout)) == DDL_OWNED_COUNT

    # A row written between the two runs: "created nothing" is only observable as
    # data that a second `CREATE` would have destroyed and did not.
    with attached(scratch) as connection:
        connection.execute(f"INSERT INTO {SCHEMA_QUALIFIER}.entities VALUES {ENTITIES_ROW}")
        before = relations(connection)

    second = run_er("init", env=scratch.env)

    assert second.returncode == 0, second.stderr
    assert actions(second.stdout) == [(relation, "exists") for relation in DDL_OWNED]
    with attached(scratch) as connection:
        assert relations(connection) == before == set(DDL_OWNED)
        row = connection.execute(f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.entities").fetchone()
        assert row is not None
        assert row[0] == 1, "the second `er init` recreated a relation that already existed"


def test_init_json_output_shape(scratch: Scratch) -> None:
    """AC1: under --json EVERY stdout line is a {relation, action} object."""
    result = run_er("init", "--json", env=scratch.env)

    assert result.returncode == 0, result.stderr
    payloads = [json.loads(line) for line in result.stdout.splitlines()]
    assert len(payloads) == DDL_OWNED_COUNT
    for payload in payloads:
        assert set(payload) == {"relation", "action"}
    assert [payload["relation"] for payload in payloads] == list(DDL_OWNED)
    assert {payload["action"] for payload in payloads} == {"created"}


def test_data_path_mismatch_exits_3_with_literal_message(scratch: Scratch) -> None:
    """AC2: a second DATA_PATH is exit 3 with the literal S4.0 message, and no change."""
    assert run_er("init", env=scratch.env).returncode == 0
    with attached(scratch) as connection:
        before = relations(connection)

    moved = DATA_PATH.format(ns=f"{scratch.ns}_moved")
    result = run_er("init", env=dict(scratch.env, ER_LAKE_DATA_PATH=moved))

    assert result.returncode == 3, result.stdout + result.stderr
    expected = DATA_PATH_MISMATCH.format(catalog=scratch.data_path, env=moved)
    assert expected in result.stderr.splitlines(), (
        f"stderr carries no line equal to the S4.0 message:\n{result.stderr}"
    )
    assert result.stdout == "", "a refused init reported relations it did not touch"
    with attached(scratch) as connection:
        assert relations(connection) == before == set(DDL_OWNED)


def test_reset_rejects_tenant_mismatch(
    scratch: Scratch, object_store: ObjectStore, catalog: CatalogConnection
) -> None:
    """AC3: `--confirm-tenant wrong` is exit 2 and destroys nothing."""
    assert run_er("init", env=scratch.env).returncode == 0
    before = fill(scratch, object_store)

    result = run_er("lake", "reset", "--confirm-tenant", "wrong", env=scratch.env)

    assert result.returncode == 2, result.stdout + result.stderr
    assert metadata_schema_exists(catalog, scratch.schema)
    assert object_store.list_prefix(scratch.data_path) == before


def test_reset_drops_schema_and_prefix_then_init_recreates(
    scratch: Scratch, object_store: ObjectStore, catalog: CatalogConnection
) -> None:
    """AC4: the confirmed reset destroys the namespace, and `er init` rebuilds it."""
    assert run_er("init", env=scratch.env).returncode == 0
    fill(scratch, object_store)

    result = run_er("lake", "reset", "--confirm-tenant", TENANT, env=scratch.env)

    assert result.returncode == 0, result.stdout + result.stderr
    assert "dropped_schema" in result.stdout
    assert "deleted_prefix" in result.stdout
    assert not metadata_schema_exists(catalog, scratch.schema)
    assert object_store.list_prefix(scratch.data_path) == []

    recreated = run_er("init", env=scratch.env)

    assert recreated.returncode == 0, recreated.stderr
    assert actions(recreated.stdout) == [(relation, "created") for relation in DDL_OWNED]
    with attached(scratch) as connection:
        assert relations(connection) == set(DDL_OWNED)


def test_reset_exits_3_when_writer_lock_held(
    scratch: Scratch, object_store: ObjectStore, catalog: CatalogConnection
) -> None:
    """AC5: reset is a writer like any other, so a held tenant lock refuses it (S4.0b)."""
    assert run_er("init", env=scratch.env).returncode == 0
    before = fill(scratch, object_store)

    # A second session holds the lock for the length of the block; the child is a
    # third session, so `pg_try_advisory_lock` refuses it rather than blocking.
    with advisory_lock(TENANT):
        result = run_er("lake", "reset", "--confirm-tenant", TENANT, env=scratch.env)

    assert result.returncode == 3, result.stdout + result.stderr
    assert metadata_schema_exists(catalog, scratch.schema)
    assert object_store.list_prefix(scratch.data_path) == before


def test_initialised_lake_fixture_has_only_ddl_owned_relations(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC7: S8.1 step 3 yields the fourteen ddl.py-owned relations and no dbt one."""
    live = relations(initialised_lake)

    assert live == set(DDL_OWNED)
    assert len(live) == DDL_OWNED_COUNT
    assert live.isdisjoint(DBT_OWNED), (
        "the dbt-owned relations are created by the first `dbt run` in the session, "
        "not by `er init` (S5.0, S8.1 step 3)"
    )
