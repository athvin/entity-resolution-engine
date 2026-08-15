"""`ddl.py` against a real DuckLake namespace (DesignDoc.md S5, S5.0, S5.1).

B2 is that the v1.0 DDL had never executed against DuckLake. So every assertion
here is made against the live catalog of the S8.1 session namespace rather than
against emitted SQL: what the registry declares and what DuckLake stores are two
different claims, and only the second one closes B2.

Each test starts from an empty namespace and leaves one behind (`empty_lake`).
That is not tidiness -- `lake_conn` is session-scoped by S8.1, `apply` is
idempotent, and a test that inherited the previous test's fourteen relations
could not tell `created` from `exists`. It also restores the precondition
`tests/integration/test_harness_namespace.py` asserts, that the harness yields a
namespace with zero relations.

The hand-made `ddl.py`-owned relations are built from the registry with one
deliberate mutation (`_handmade`), so a test's *only* difference from S5 is the
one it is about; a hand-written column list would drift from S5 and start failing
for a second reason.
"""

from __future__ import annotations

from collections.abc import Iterator, Mapping

import duckdb
import pytest

from er.lake.ddl import (
    ERR_SCHEMA_BREAKING,
    ColumnDiff,
    OwnershipViolation,
    RelationAction,
    SchemaBreakingError,
    apply,
    assert_ddl_owned,
    create_statement,
    diff,
    evolve,
    live_columns,
)
from er.lake.model import DBT_OWNED, DDL_OWNED, REGISTRY, SCHEMA_QUALIFIER

# S5.0's ownership table lists exactly this many `ddl.py`-owned relations, and
# S4.0 makes `er init` print one line per relation. Spelled as a literal because a
# count derived from `DDL_OWNED` would agree with the registry no matter what the
# registry said.
DDL_OWNED_COUNT = 14

# The constraints DuckLake supports none of (S5.0). `NOT NULL` is deliberately
# absent: it is the one constraint a generated statement may carry.
FORBIDDEN_CONSTRAINTS = ("PRIMARY KEY", "UNIQUE", "FOREIGN KEY", "CHECK", "DEFAULT", "CREATE INDEX")

# The dbt-owned relation of AC3, given a column set that is wrong in both
# directions: a column S5 does not declare, and every column S5 does declare but
# this one missing. `ddl.py` must still leave it exactly as found.
WRONG_INT_STD_RECORDS = "record_key VARCHAR, bogus_column BIGINT"

_DATABASE, _SCHEMA = SCHEMA_QUALIFIER.split(".", 1)


def _relations_in_lake(connection: duckdb.DuckDBPyConnection) -> set[str]:
    rows = connection.execute(
        "SELECT table_name FROM duckdb_tables() WHERE database_name = ? AND schema_name = ?",
        [_DATABASE, _SCHEMA],
    ).fetchall()
    return {str(name) for (name,) in rows}


def _drop_every_relation(connection: duckdb.DuckDBPyConnection) -> None:
    """Return the namespace to the empty state S8.1 hands a session."""
    for name in sorted(_relations_in_lake(connection)):
        connection.execute(f"DROP TABLE {SCHEMA_QUALIFIER}.{name}")


@pytest.fixture(autouse=True)
def empty_lake(lake_conn: duckdb.DuckDBPyConnection) -> Iterator[duckdb.DuckDBPyConnection]:
    """An empty namespace before the test, and an empty one after it."""
    _drop_every_relation(lake_conn)
    try:
        yield lake_conn
    finally:
        _drop_every_relation(lake_conn)


def _handmade(
    connection: duckdb.DuckDBPyConnection,
    relation: str,
    *,
    without: str = "",
    retyped: Mapping[str, str] | None = None,
) -> None:
    """Create ``relation`` from its registry spec, minus or re-typing one column.

    Nullability is preserved from the spec so that the mutation named by the
    caller is the only way the live relation differs from S5.
    """
    types = dict(retyped or {})
    columns = [
        f"{column.name} {types.get(column.name, column.type)}"
        f"{'' if column.nullable else ' NOT NULL'}"
        for column in REGISTRY[relation].columns
        if column.name != without
    ]
    connection.execute(f"CREATE TABLE {SCHEMA_QUALIFIER}.{relation} ({', '.join(columns)})")


def _row_count(connection: duckdb.DuckDBPyConnection, relation: str) -> int:
    row = connection.execute(f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{relation}").fetchone()
    assert row is not None
    return int(row[0])


def _columns(connection: duckdb.DuckDBPyConnection, relation: str) -> dict[str, str]:
    return dict(live_columns(connection, relation))


def test_apply_creates_the_fourteen_ddl_owned_relations(
    lake_conn: duckdb.DuckDBPyConnection,
) -> None:
    results = apply(lake_conn)

    assert len(DDL_OWNED) == DDL_OWNED_COUNT
    assert [result.relation for result in results] == list(DDL_OWNED)
    assert {result.action for result in results} == {RelationAction.CREATED}
    assert _relations_in_lake(lake_conn) == set(DDL_OWNED)

    for relation in DDL_OWNED:
        # Queryable and empty: the relation exists in DuckLake, not merely in the
        # statement text a `CREATE` was built from (B2).
        assert _row_count(lake_conn, relation) == 0
        assert _columns(lake_conn, relation) != {}

    for relation in DBT_OWNED:
        assert _columns(lake_conn, relation) == {}, (
            f"{relation} is dbt-owned and is created by the first `dbt run` (S5.0)"
        )


def test_apply_is_idempotent(lake_conn: duckdb.DuckDBPyConnection) -> None:
    apply(lake_conn)
    lake_conn.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.entities "
        "VALUES ('E1', 'active', NULL, now(), now(), 'R1', 'R1')"
    )
    before = {relation: _columns(lake_conn, relation) for relation in DDL_OWNED}
    counts = {relation: _row_count(lake_conn, relation) for relation in DDL_OWNED}

    results = apply(lake_conn)

    assert {result.action for result in results} == {RelationAction.EXISTS}
    assert [result.statement for result in results] == [None] * DDL_OWNED_COUNT
    assert {relation: _columns(lake_conn, relation) for relation in DDL_OWNED} == before
    assert {relation: _row_count(lake_conn, relation) for relation in DDL_OWNED} == counts
    assert counts["entities"] == 1


def test_apply_never_touches_dbt_owned_relations(lake_conn: duckdb.DuckDBPyConnection) -> None:
    lake_conn.execute(f"CREATE TABLE {SCHEMA_QUALIFIER}.int_std_records ({WRONG_INT_STD_RECORDS})")
    lake_conn.execute(f"INSERT INTO {SCHEMA_QUALIFIER}.int_std_records VALUES ('crm:1', 7)")
    before = _columns(lake_conn, "int_std_records")
    assert before == {"record_key": "VARCHAR", "bogus_column": "BIGINT"}

    results = apply(lake_conn)

    assert "int_std_records" not in [result.relation for result in results]
    assert _columns(lake_conn, "int_std_records") == before
    rows = lake_conn.execute(f"SELECT * FROM {SCHEMA_QUALIFIER}.int_std_records").fetchall()
    assert rows == [("crm:1", 7)]

    with pytest.raises(OwnershipViolation):
        create_statement("int_std_records")
    with pytest.raises(OwnershipViolation):
        assert_ddl_owned("int_std_records")


def test_evolve_adds_nullable_column_only(lake_conn: duckdb.DuckDBPyConnection) -> None:
    # `raw_records.deleted_at` is nullable in S5 and is the M15 arm that the
    # deletion columns exist from M1, so it is the column an older lake would lack.
    _handmade(lake_conn, "raw_records", without="deleted_at")
    lake_conn.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.raw_records "
        "VALUES ('crm', '1', '{}', 'h', false, 'B1', now())"
    )

    statements = evolve(lake_conn)

    assert len(statements) == 1
    statement = statements[0]
    assert statement.startswith(f"ALTER TABLE {SCHEMA_QUALIFIER}.raw_records ")
    assert "ADD COLUMN deleted_at TIMESTAMP" in statement
    assert "NOT NULL" not in statement
    assert "DEFAULT" not in statement

    assert "deleted_at" in _columns(lake_conn, "raw_records")
    deleted = lake_conn.execute(f"SELECT deleted_at FROM {SCHEMA_QUALIFIER}.raw_records").fetchall()
    assert deleted == [(None,)], "the added column must be NULL on the pre-existing row"
    # Nothing left to reconcile, so the second call is the S5.1 no-op.
    assert evolve(lake_conn) == ()


def test_breaking_type_change_raises_err_schema_breaking(
    lake_conn: duckdb.DuckDBPyConnection,
) -> None:
    # `runs.snapshot_start` is BIGINT in S5; a VARCHAR here is the re-typed column
    # S5.1 refuses to migrate.
    _handmade(lake_conn, "runs", retyped={"snapshot_start": "VARCHAR"})
    before = _columns(lake_conn, "runs")

    with pytest.raises(SchemaBreakingError) as raised:
        evolve(lake_conn)

    expected = f"{ERR_SCHEMA_BREAKING}: runs.snapshot_start: VARCHAR -> BIGINT"
    assert str(raised.value) == expected
    assert raised.value.code == 3
    # No `ALTER` ran: the live shape is what it was before the call.
    assert _columns(lake_conn, "runs") == before
    assert ColumnDiff("runs", "snapshot_start", "VARCHAR", "BIGINT") in diff(lake_conn)


def test_undeclared_live_column_is_breaking(lake_conn: duckdb.DuckDBPyConnection) -> None:
    apply(lake_conn)
    lake_conn.execute(f"ALTER TABLE {SCHEMA_QUALIFIER}.entities ADD COLUMN legacy_flag BOOLEAN")
    before = _columns(lake_conn, "entities")

    with pytest.raises(SchemaBreakingError) as raised:
        evolve(lake_conn)

    prefix = f"{ERR_SCHEMA_BREAKING}: entities.legacy_flag: BOOLEAN -> "
    assert str(raised.value).startswith(prefix)
    assert raised.value.code == 3
    assert _columns(lake_conn, "entities") == before

    difference = raised.value.difference
    assert difference.breaking
    assert not difference.additive
    assert difference.declared_type is None


def test_emitted_statements_declare_only_not_null(lake_conn: duckdb.DuckDBPyConnection) -> None:
    statements = [result.statement for result in apply(lake_conn)]

    assert len(statements) == DDL_OWNED_COUNT
    for statement in statements:
        assert statement is not None
        upper = statement.upper()
        assert upper.startswith("CREATE TABLE IF NOT EXISTS ")
        for constraint in FORBIDDEN_CONSTRAINTS:
            assert constraint not in upper, f"{constraint} in:\n{statement}"
        # `NOT NULL` is the one constraint DuckLake enforces, and S5 declares it on
        # at least one column of every `ddl.py`-owned relation.
        assert "NOT NULL" in upper
