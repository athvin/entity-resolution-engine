"""The second half of the S8.1 isolation contract (DesignDoc.md S8.1, S5, S5.0).

ER-018 gave the session a namespace so that one *suite* cannot feed the next.
This module is about the other half of M22: one *test* feeding the next. A suite
that leaves a relation populated makes test ordering load-bearing, and the
symptom is never the test that caused it.

Four of the criteria here are not observable from inside the session that makes
them. "Teardown reclaimed the sub-namespaces" cannot be asserted by the test
whose teardown is being tested; "``--keep-lake`` left the namespace behind"
cannot be asserted by the session that would otherwise have reclaimed it; and a
*collection* failure is by definition not reachable from a test that was itself
collected. Those four drive a **nested pytest session** in a subprocess, handed
the real `tests/conftest.py` with ``-p conftest`` over a ``PYTHONPATH`` carrying
``tests/`` -- a copy would test a copy. The outer session then asserts, from its
own catalog connection and its own S3 client, what the inner one left behind.

Two pairs of tests here are deliberately *ordered*: a test that dirties the
namespace, followed by a test that asserts `clean_lake` emptied it. That is what
"function-isolated" means and there is no way to assert it within one test --
the fixture under test runs between them. Both pairs are adjacent and in file
order, which is the order pytest runs them in.

S8.1's namespace shape (`er_test_<ns>`, `s3://lake/test/<ns>/`) is spelled here
as literals, for the same reason `test_harness_namespace.py` spells it: a test
that took the constant from the implementation could not catch a wrong constant.
The *relation lists* are the opposite case and come from the harness itself,
because AC1's claim is precisely that they are derived from the S5 registry
rather than carried as a list of their own. They arrive as the
`isolation_relations` fixture rather than as an import: three files in this tree
are named `conftest`, pytest imports each under that one module name, and
`import conftest` resolves to whichever it loaded last.

Nothing here asserts a snapshot count or an absolute snapshot version (S8.1, the
S4 preamble); the guard's own probe modules are assembled from fragments so that
this file stays collectable under the rule it enforces.
"""

from __future__ import annotations

import os
import subprocess
import sys
from collections.abc import Callable, Iterator, Mapping
from pathlib import Path
from typing import TYPE_CHECKING

import duckdb
import pytest

from er.lake.catalog import CatalogConnection, drop_metadata_schema, metadata_schema_exists
from er.lake.init import init_lake
from er.lake.model import (
    COLUMN_TYPES,
    DBT_OWNED,
    DDL_OWNED,
    REGISTRY,
    SCHEMA_QUALIFIER,
    Owner,
    TableSpec,
)
from er.lake.objectstore import ObjectStore

if TYPE_CHECKING:  # annotations only -- never executed, so the name clash cannot bite
    from conftest import LakeNamespace, SubNamespace

#: S8.1 step 4's opt-out. Spelled rather than imported, with the namespace shape.
KEEP_LAKE_OPTION = "--keep-lake"

# S8.1's parenthetical list of the relations function isolation empties, transcribed
# as literals. The *fixture* must derive its list from the registry -- a hard-coded
# fourteen rots the first time S5 grows -- and this is the one place that checks the
# registry still says what S8.1 says. A count derived from `DDL_OWNED` would agree
# with the registry no matter what the registry said.
S8_1_DDL_OWNED = (
    "raw_records",
    "match_scores",
    "entity_membership",
    "entities",
    "entity_events",
    "assertions",
    "review_queue",
    "model_registry",
    "tf_lookup",
    "cut_edges",
    "runs",
    "run_stages",
    "ingest_batches",
    "er_touched_entities",
)

#: One dbt-owned relation standing for the set. Created by hand rather than by a
#: `dbt run`: what AC2 is about is that a dbt-owned relation left behind is *dropped*
#: and not merely emptied, and how it came to exist is not part of that claim.
DBT_PROBE = "int_std_records"

# One writable literal per S5 type (S5.0 permits exactly eight, and `NOT NULL` is the
# only constraint any of these relations carries), so a row can be written into every
# `ddl.py`-owned relation straight from its registry spec. A hand-written row per
# relation would drift from S5 and start failing for a second reason.
VALUE_BY_TYPE: Mapping[str, str] = {
    "VARCHAR": "'x'",
    "BIGINT": "1",
    "DOUBLE": "1.0",
    "BOOLEAN": "true",
    "DATE": "DATE '2026-01-01'",
    "TIMESTAMP": "TIMESTAMP '2026-01-01 00:00:00'",
    "JSON": "'{}'::JSON",
    "LIST(VARCHAR)": "['x']",
}

# S8.1 names the two universes T-INC-1 builds; the outer session re-derives both
# names from the namespace a nested session hands back.
SUB_SUFFIXES = ("a", "b")

# Where the harness lives, put on the nested session's `PYTHONPATH` so `-p conftest`
# resolves to the real module rather than to a copy.
TESTS_DIR = Path(__file__).resolve().parents[1]

# How a nested session hands its namespace back. A file rather than stdout parsing:
# `-q` output is pytest's format to change, and the namespace is the one value the
# outer assertions cannot re-derive.
NS_HANDBACK_ENV = "ER_HARNESS_NS_FILE"

_DATABASE, _SCHEMA = SCHEMA_QUALIFIER.split(".", 1)


# --------------------------------------------------------------------------- #
# reading the live namespace
# --------------------------------------------------------------------------- #


def _relations(connection: duckdb.DuckDBPyConnection) -> set[str]:
    rows = connection.execute(
        "SELECT table_name FROM duckdb_tables() WHERE database_name = ? AND schema_name = ?",
        [_DATABASE, _SCHEMA],
    ).fetchall()
    return {str(name) for (name,) in rows}


def _row_count(connection: duckdb.DuckDBPyConnection, relation: str) -> int:
    row = connection.execute(f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{relation}").fetchone()
    assert row is not None
    return int(row[0])


def _insert_one(connection: duckdb.DuckDBPyConnection, spec: TableSpec) -> None:
    """Write one row into ``spec``'s relation, built from its declared column types."""
    values = ", ".join(VALUE_BY_TYPE[column.type] for column in spec.columns)
    connection.execute(f"INSERT INTO {SCHEMA_QUALIFIER}.{spec.name} VALUES ({values})")


def _schemas_like(connection: CatalogConnection, pattern: str) -> list[str]:
    rows = connection.execute(
        "SELECT schema_name FROM information_schema.schemata WHERE schema_name LIKE %s ORDER BY 1",
        (pattern,),
    ).fetchall()
    return [str(name) for (name,) in rows]


@pytest.fixture(scope="module", autouse=True)
def restored_namespace(lake_conn: duckdb.DuckDBPyConnection) -> Iterator[None]:
    """An empty namespace before this module and an empty one after it.

    Function isolation *empties* the `ddl.py`-owned relations rather than dropping
    them, which is the contract -- and which means the tests below leave fourteen
    relations standing. `test_harness_namespace.py` asserts the harness yields a
    namespace with zero relations, so this module restores that precondition itself
    rather than making the two files order-dependent.
    """
    for name in sorted(_relations(lake_conn)):
        lake_conn.execute(f"DROP TABLE {SCHEMA_QUALIFIER}.{name}")
    try:
        yield
    finally:
        for name in sorted(_relations(lake_conn)):
            lake_conn.execute(f"DROP TABLE {SCHEMA_QUALIFIER}.{name}")


# --------------------------------------------------------------------------- #
# nested sessions
# --------------------------------------------------------------------------- #

# Builds both universes S8.1 names, writes a relation *and* an object into each --
# they are reclaimed by different steps of step 4, the Parquet by
# `cleanup_old_files` plus the prefix delete, the catalog rows by the `DROP SCHEMA`
# -- and asserts it wrote something, so "teardown reclaimed it" cannot pass vacuously.
_NESTED_SUB_FILLS = '''\
"""A nested session that builds two sub-namespaces from the S8.1 harness."""

import os
from pathlib import Path

PROBE_TABLE = "lake.main.sub_namespace_probe"


def _fill(lake_ns, sub_namespace, object_store):
    Path(os.environ["__HANDBACK__"]).write_text(lake_ns, encoding="utf-8")
    for suffix in ("a", "b"):
        universe = sub_namespace(suffix)
        universe.connection.execute(f"CREATE TABLE {PROBE_TABLE} (n INTEGER)")
        universe.connection.execute(f"INSERT INTO {PROBE_TABLE} VALUES (1)")
        object_store.put_bytes(universe.namespace.data_path + "probe.txt", b"probe")
        assert object_store.list_prefix(universe.namespace.data_path), (
            "the nested session wrote nothing, so teardown would have nothing to reclaim"
        )
'''

NESTED_SUB_PASSES = (
    _NESTED_SUB_FILLS
    + """

def test_builds_two_universes(lake_ns, sub_namespace, object_store):
    _fill(lake_ns, sub_namespace, object_store)
"""
)

NESTED_SUB_FAILS = (
    _NESTED_SUB_FILLS
    + """

def test_builds_two_universes_then_raises(lake_ns, sub_namespace, object_store):
    _fill(lake_ns, sub_namespace, object_store)
    raise AssertionError("deliberate failure: teardown must still reclaim both universes")
"""
)

# The session namespace rather than a sub-namespace, because `--keep-lake` is a
# property of S8.1 step 4 wherever it runs and the session fixture is where an
# operator meets it.
NESTED_SESSION_FILLS = '''\
"""A nested session that takes a namespace from the S8.1 harness and fills it."""

import os
from pathlib import Path

PROBE_TABLE = "lake.main.keep_lake_probe"


def test_fills_the_namespace(lake_ns, er_env, lake_conn, object_store):
    Path(os.environ["__HANDBACK__"]).write_text(lake_ns, encoding="utf-8")
    lake_conn.execute(f"CREATE TABLE {PROBE_TABLE} (n INTEGER)")
    lake_conn.execute(f"INSERT INTO {PROBE_TABLE} VALUES (1)")
    object_store.put_bytes(er_env.data_path + "probe.txt", b"probe")
    assert object_store.list_prefix(er_env.data_path), (
        "the nested session wrote nothing, so there would be nothing to retain"
    )
'''


def _run_nested(
    tmp_path: Path,
    module: Path,
    *,
    args: tuple[str, ...] = (),
    extra_env: Mapping[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run ``module`` as its own pytest session over the real harness."""
    # The nested run inherits the *outer* namespace in its environment; its own
    # `er_env` overwrites the three namespace variables before anything connects,
    # which is what makes the two sessions independent rather than nested.
    environment = dict(os.environ)
    environment["PYTHONPATH"] = os.pathsep.join(
        [str(TESTS_DIR), *([environment["PYTHONPATH"]] if environment.get("PYTHONPATH") else [])]
    )
    environment.update(extra_env or {})
    return subprocess.run(
        [sys.executable, "-m", "pytest", str(module), "-q", "-p", "conftest", *args],
        # From `tmp_path`, so the nested rootdir holds no `conftest.py` of its own and
        # `-p conftest` cannot resolve to anything but the harness.
        cwd=tmp_path,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )


def _nested_with_handback(
    tmp_path: Path, body: str, *, args: tuple[str, ...] = ()
) -> tuple[subprocess.CompletedProcess[str], str]:
    """Run ``body`` as a one-test session; return it with the namespace it minted."""
    module = tmp_path / "test_nested_isolation.py"
    module.write_text(body.replace("__HANDBACK__", NS_HANDBACK_ENV), encoding="utf-8")
    handback = tmp_path / "namespace.txt"
    # Removed rather than overwritten: a second nested run in the same `tmp_path` that
    # died before reaching the harness would otherwise hand back the first run's
    # namespace, and the assertions would pass against the wrong one.
    handback.unlink(missing_ok=True)

    completed = _run_nested(tmp_path, module, args=args, extra_env={NS_HANDBACK_ENV: str(handback)})
    assert handback.is_file(), (
        "the nested session never reached the harness:\n" + completed.stdout + completed.stderr
    )
    return completed, handback.read_text(encoding="utf-8").strip()


def _assert_sub_namespaces_reclaimed(
    store: ObjectStore, catalog_connection: CatalogConnection, ns: str
) -> None:
    """Neither `er_test_<ns>_a` nor `er_test_<ns>_b` survived, in catalog or in S3."""
    for suffix in SUB_SUFFIXES:
        schema = f"er_test_{ns}_{suffix}"
        prefix = f"s3://lake/test/{ns}_{suffix}/"
        assert not metadata_schema_exists(catalog_connection, schema), (
            f"teardown left the catalog schema {schema} behind"
        )
        keys = store.list_prefix(prefix)
        assert keys == [], f"teardown left {len(keys)} object(s) under {prefix}: {keys[:5]}"

    # Not just the two names this test expects: zero schemas of *any* suffix, so a
    # sub-namespace built under a name the outer session never guessed still fails.
    survivors = [name for name in _schemas_like(catalog_connection, f"er_test_{ns}%")]
    assert survivors == [], f"teardown left {survivors} behind for namespace {ns}"


# --------------------------------------------------------------------------- #
# AC1 / AC2 -- function isolation
# --------------------------------------------------------------------------- #


def test_every_ddl_owned_relation_receives_a_row(lake_conn: duckdb.DuckDBPyConnection) -> None:
    """The dirtying half of AC1: the next test is the assertion."""
    assert set(VALUE_BY_TYPE) == COLUMN_TYPES, (
        "S5 declares a column type this module cannot write a row of"
    )

    init_lake(connection=lake_conn)
    assert _relations(lake_conn) >= set(DDL_OWNED)

    for relation in DDL_OWNED:
        _insert_one(lake_conn, REGISTRY[relation])
        assert _row_count(lake_conn, relation) == 1


def test_function_isolation_empties_every_ddl_owned_relation(
    lake_conn: duckdb.DuckDBPyConnection,
) -> None:
    live = _relations(lake_conn)
    assert live >= set(DDL_OWNED), (
        "function isolation dropped the `ddl.py`-owned relations instead of emptying "
        "them; the next test's `er init` could then not tell `created` from `exists`"
    )
    counts = {relation: _row_count(lake_conn, relation) for relation in DDL_OWNED}
    assert counts == dict.fromkeys(DDL_OWNED, 0), (
        f"the previous test's rows survived into this one: "
        f"{ {name: n for name, n in counts.items() if n} }"
    )


def test_delete_list_is_derived_from_registry(
    isolation_relations: tuple[tuple[str, ...], tuple[str, ...]],
) -> None:
    delete_relations, drop_relations = isolation_relations

    assert set(delete_relations) == {
        spec.name for spec in REGISTRY.values() if spec.owner is Owner.DDL
    }
    assert set(drop_relations) == {
        spec.name for spec in REGISTRY.values() if spec.owner is Owner.DBT
    }
    # A relation has exactly one owner (S5.0, D14), so the two halves cannot overlap:
    # a relation both emptied and dropped would be dropped, and the next `er init`
    # would recreate a relation dbt owns.
    assert set(delete_relations).isdisjoint(drop_relations)

    # ... and the registry still agrees with S8.1's own list.
    assert set(delete_relations) == set(S8_1_DDL_OWNED)
    assert len(S8_1_DDL_OWNED) == len(set(S8_1_DDL_OWNED)) == 14


def test_a_dbt_owned_relation_is_created(lake_conn: duckdb.DuckDBPyConnection) -> None:
    """The dirtying half of AC2: the next test is the assertion."""
    lake_conn.execute(f"CREATE TABLE {SCHEMA_QUALIFIER}.{DBT_PROBE} (record_key VARCHAR)")
    lake_conn.execute(f"INSERT INTO {SCHEMA_QUALIFIER}.{DBT_PROBE} VALUES ('crm:1')")

    assert DBT_PROBE in _relations(lake_conn)


def test_dbt_owned_relations_are_dropped_between_tests(
    lake_conn: duckdb.DuckDBPyConnection,
) -> None:
    live = _relations(lake_conn)

    assert DBT_PROBE not in live, (
        f"{DBT_PROBE} survived into this test; the next `dbt run` would materialize "
        "a relation that already holds another test's rows"
    )
    assert live.isdisjoint(DBT_OWNED)
    # The asymmetry is the point: dbt-owned relations go, `ddl.py`-owned ones stay.
    assert live >= set(DDL_OWNED)


# --------------------------------------------------------------------------- #
# AC3 / AC4 -- sub-namespaces
# --------------------------------------------------------------------------- #


def test_sub_namespaces_are_independent_and_reclaimed(
    tmp_path: Path,
    sub_namespace: Callable[[str], SubNamespace],
    er_env: LakeNamespace,
    catalog: CatalogConnection,
    object_store: ObjectStore,
) -> None:
    universe_a = sub_namespace("a")
    universe_b = sub_namespace("b")

    assert universe_a.namespace.metadata_schema == f"er_test_{er_env.ns}_a"
    assert universe_b.namespace.metadata_schema == f"er_test_{er_env.ns}_b"
    assert universe_a.namespace.data_path == f"s3://lake/test/{er_env.ns}_a/"
    assert universe_b.namespace.data_path == f"s3://lake/test/{er_env.ns}_b/"
    assert universe_a.namespace.metadata_schema != universe_b.namespace.metadata_schema
    assert universe_a.namespace.data_path != universe_b.namespace.data_path
    assert metadata_schema_exists(catalog, universe_a.namespace.metadata_schema)
    assert metadata_schema_exists(catalog, universe_b.namespace.metadata_schema)

    # S8.1 step 3 ran in each on entry, which is what makes them usable universes
    # rather than two empty namespaces.
    for universe in (universe_a, universe_b):
        assert _relations(universe.connection) >= set(DDL_OWNED)

    _insert_one(universe_a.connection, REGISTRY["raw_records"])
    assert _row_count(universe_a.connection, "raw_records") == 1
    assert _row_count(universe_b.connection, "raw_records") == 0, (
        "the two universes share one lake; T-INC-1 would compare a universe with itself"
    )

    # Each hands a subprocess its own environment, and neither re-points the session:
    # a `sub_namespace` that exported would make the *next* `er init` of this test
    # apply itself to the wrong lake.
    assert universe_a.env["ER_LAKE_METADATA_SCHEMA"] == universe_a.namespace.metadata_schema
    assert universe_a.env["ER_LAKE_DATA_PATH"] == universe_a.namespace.data_path
    assert os.environ["ER_LAKE_METADATA_SCHEMA"] == er_env.metadata_schema
    assert os.environ["ER_LAKE_DATA_PATH"] == er_env.data_path

    with pytest.raises(ValueError, match="already exists"):
        sub_namespace("a")

    # Reclamation is the half this test cannot observe about itself: the fixture that
    # would prove it is the fixture still holding both universes open.
    completed, nested_ns = _nested_with_handback(tmp_path, NESTED_SUB_PASSES)
    assert completed.returncode == 0, completed.stdout + completed.stderr
    assert nested_ns != er_env.ns, "the nested session reused the outer session's namespace"
    _assert_sub_namespaces_reclaimed(object_store, catalog, nested_ns)


def test_teardown_runs_after_a_failing_test(
    tmp_path: Path, catalog: CatalogConnection, object_store: ObjectStore, er_env: LakeNamespace
) -> None:
    completed, nested_ns = _nested_with_handback(tmp_path, NESTED_SUB_FAILS)

    assert completed.returncode != 0, "the deliberately failing nested session reported success"
    assert nested_ns != er_env.ns
    # The failing variant is the one that matters: a harness that reclaimed only on
    # the happy path would leak two schemas and two prefixes on every red build.
    _assert_sub_namespaces_reclaimed(object_store, catalog, nested_ns)


# --------------------------------------------------------------------------- #
# AC5 -- --keep-lake
# --------------------------------------------------------------------------- #


def test_keep_lake_suppresses_teardown(
    tmp_path: Path, catalog: CatalogConnection, object_store: ObjectStore
) -> None:
    kept, kept_ns = _nested_with_handback(tmp_path, NESTED_SESSION_FILLS, args=(KEEP_LAKE_OPTION,))
    assert kept.returncode == 0, kept.stdout + kept.stderr

    schema = f"er_test_{kept_ns}"
    prefix = f"s3://lake/test/{kept_ns}/"
    try:
        output = kept.stdout + kept.stderr
        assert kept_ns in output, f"{KEEP_LAKE_OPTION} did not print the retained namespace"
        assert schema in output
        assert metadata_schema_exists(catalog, schema), (
            f"{KEEP_LAKE_OPTION} reclaimed the namespace it promised to retain"
        )
        assert object_store.list_prefix(prefix) != []
    finally:
        # Whoever asks for a retained namespace owns it, and here that is this test.
        # Under `finally` because a failed assertion above must not also leak.
        object_store.delete_prefix(prefix)
        drop_metadata_schema(catalog, schema)

    dropped, dropped_ns = _nested_with_handback(tmp_path, NESTED_SESSION_FILLS)
    assert dropped.returncode == 0, dropped.stdout + dropped.stderr
    assert dropped_ns != kept_ns, "two nested sessions minted the same namespace"

    assert not metadata_schema_exists(catalog, f"er_test_{dropped_ns}"), (
        "without --keep-lake the namespace must be reclaimed"
    )
    assert object_store.list_prefix(f"s3://lake/test/{dropped_ns}/") == []
    assert dropped_ns not in (dropped.stdout + dropped.stderr)


# --------------------------------------------------------------------------- #
# AC7 -- the snapshot-literal collection guard
# --------------------------------------------------------------------------- #

# Assembled from fragments rather than written out: this module lives under
# `tests/integration/`, so the guard it exercises would -- correctly -- refuse to
# collect a file that spelled an absolute version. The two clean forms below need no
# such care, which is itself the distinction the guard draws.
ABSOLUTE_VERSION = "AT (VERSION => " + "118)"
LITERAL_COMPARISON = "WHERE snapshot_" + "start >= 118"
RUNTIME_VERSION = "AT (VERSION => :snap)"

# The offending fragment lands on line 3, which the refusal must name.
VIOLATION_LINE = 3

GUARD_PROBE = '''\
"""A module the S8.1 collection guard must judge."""

SQL = "SELECT * FROM lake.main.golden_records __CLAUSE__"


def test_reads_golden_records():
    assert "golden_records" in SQL
'''


def _guard_probe(tmp_path: Path, clause: str) -> Path:
    """Write the probe under a `tests/integration/` of its own and return its path.

    The guard matches on the *relative* parts `tests/integration`, not on this
    repository's absolute path, which is what lets its self-test build a violating
    module here instead of committing one -- a committed one would fail this very
    check.
    """
    module = tmp_path / "tests" / "integration" / "test_snapshot_probe.py"
    module.parent.mkdir(parents=True, exist_ok=True)
    module.write_text(GUARD_PROBE.replace("__CLAUSE__", clause), encoding="utf-8")
    return module


@pytest.mark.parametrize("clause", [ABSOLUTE_VERSION, LITERAL_COMPARISON])
def test_snapshot_literal_guard_fails_collection(tmp_path: Path, clause: str) -> None:
    module = _guard_probe(tmp_path, clause)

    refused = _run_nested(tmp_path, module)
    output = refused.stdout + refused.stderr

    assert refused.returncode != 0, output
    assert f"{module.name}:{VIOLATION_LINE}:" in output, (
        f"the refusal named neither the path nor the line:\n{output}"
    )
    assert str(module) in output
    assert "no test may reference an absolute snapshot version" in output
    # Collection failed, so the test inside was never run.
    assert "1 passed" not in output


def test_runtime_bound_snapshot_collects_cleanly(tmp_path: Path) -> None:
    module = _guard_probe(tmp_path, RUNTIME_VERSION)

    collected = _run_nested(tmp_path, module)
    output = collected.stdout + collected.stderr

    assert collected.returncode == 0, output
    assert "1 passed" in output
    # The sanctioned form of the very same query: same file, same line, bound at
    # runtime instead of pinned.
    assert "no test may reference an absolute snapshot version" not in output
