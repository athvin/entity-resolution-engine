"""M17, in executable form: Splink's intermediates never reach the lake (S4.0b).

The claim `splink_api()` makes is not "Splink writes somewhere tidy". It is that a
Splink run against an attached DuckLake commits **no snapshot at all** — which is a
claim about the lake's snapshot log, not about a schema name, and the only way to
check it is to run a real prediction against a real lake and look. So every test here
does that, and each one runs in its **own sub-namespace**: a leak is a write to a
catalog schema and an object-store prefix, and a leak into the session namespace
would be reclaimed by the session's own teardown before any later test could see it.

The negative arm is the reason to believe the positive one. `output_schema` alone
does not deliver the guarantee: Splink's `SET schema` names a schema, not a catalog,
so with DuckLake as the default catalog the scratch schema is created *in the lake*
and the intermediates follow. :func:`test_assertion_fails_on_misconfigured_api`
builds exactly that connection, materializes one Splink intermediate through it, and
asserts the guard fires — without it, five green tests would be consistent with an
assertion that can only ever pass.

The frame every test scores is built here rather than read from
`lake.main.int_std_records`: this ticket precedes the intermediate models, and a
20-row frame in the in-memory database is also the S4.0b-sanctioned shape for a
Splink input ("materialized into local temp tables first"). It is referenced by a
**bare** name because Splink resolves an input table's columns with
`information_schema.columns WHERE table_name = '<name>'` — a schema-qualified name
matches no row there, and the failure surfaces as every column being reported
missing from the settings.
"""

from __future__ import annotations

import os
from collections.abc import Iterator, Sequence
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from splink import DuckDBAPI, Linker

from er.config.loader import load_config
from er.config.schema import Config
from er.errors import StageFailure
from er.lake.catalog import CatalogConnection, metadata_schema_exists
from er.lake.ducklake import LAKE_ALIAS, current_snapshot
from er.matching.api import (
    SPLINK_RELATION_PREFIX,
    SPLINK_SCRATCH_SCHEMA,
    assert_no_splink_relations_in_lake,
    leaked_splink_relations,
    splink_api,
)
from er.matching.model import build_settings

# `tests/conftest.py`'s `SubNamespace` is deliberately NOT imported: `pythonpath =
# ["tests"]` and three files in this tree are named `conftest`, so `import conftest`
# resolves to whichever pytest loaded last. The fixture hands the object over; its
# type is spelled `Any` here rather than dragged in through an unreliable import.

#: The `LIKE` pattern S8.3 spells for T-DOCTOR-1 and T-INV-1, built from the module
#: under test so the two cannot drift.
SPLINK_PATTERN: Final[str] = f"{SPLINK_RELATION_PREFIX}%"

#: The local frame, bare-named for the reason in the module docstring.
FRAME: Final[str] = "er_049_frame"

#: Only the `int_std_records` columns `configs/test.yaml` actually references — its
#: four `blocking:` expressions and its six `comparisons:` keys, plus `record_key` as
#: `unique_id_column_name`. `birth_date` is a real `DATE`: the `dob_same_year_month`
#: level renders `date_trunc('month', birth_date_l)`, which has no VARCHAR overload.
FRAME_DDL: Final[str] = f"""
CREATE TABLE {FRAME} (
    record_key    VARCHAR,
    given_name    VARCHAR,
    family_name   VARCHAR,
    name_variants VARCHAR[],
    email         VARCHAR,
    phone_e164    VARCHAR,
    birth_date    DATE,
    addr_postal   VARCHAR
)
"""

#: Ten personas, two records each. Twenty rows is what AC2 names, and the pairing is
#: what makes the prediction non-empty: a run that scored nothing would satisfy "zero
#: `__splink__` relations in the lake" vacuously.
PERSONA_COUNT: Final[int] = 10

#: S4.3.5's `review_low` band starts here, and `configs/test.yaml` sets it. Passed to
#: `predict` so the two-record personas survive the threshold.
REVIEW_LOW: Final[float] = 0.60


def frame_rows() -> list[tuple[Any, ...]]:
    """Twenty rows forming ten exact-duplicate pairs, in `record_key` order."""
    rows: list[tuple[Any, ...]] = []
    for index in range(PERSONA_COUNT * 2):
        persona = index // 2
        rows.append(
            (
                f"crm:{index:02d}",
                f"person{persona}",
                f"sur{persona}",
                [f"person{persona}"],
                f"p{persona}@example.com",
                f"+1555000{persona:04d}",
                f"1990-01-{persona % 9 + 1:02d}",
                f"{10000 + persona}",
            )
        )
    return rows


def materialize_frame(connection: duckdb.DuckDBPyConnection) -> None:
    """Build :data:`FRAME` wherever the connection's search path currently points."""
    connection.execute(FRAME_DDL)
    connection.executemany(f"INSERT INTO {FRAME} VALUES (?, ?, ?, ?, ?, ?, ?, ?)", frame_rows())


def scalar(
    connection: duckdb.DuckDBPyConnection, statement: str, parameters: Sequence[Any] | None = None
) -> Any:
    row = connection.execute(statement, parameters).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def splink_relation_count(connection: duckdb.DuckDBPyConnection, database: str) -> int:
    """How many `__splink__` relations live in `database` on this connection."""
    return int(
        scalar(
            connection,
            "SELECT count(*) FROM duckdb_tables() WHERE database_name = ? AND table_name LIKE ?",
            [database, SPLINK_PATTERN],
        )
    )


def predict_pairs(connection: duckdb.DuckDBPyConnection, cfg: Config) -> list[tuple[Any, ...]]:
    """One S4.3.4 `--mode full` prediction, read back **in the order Splink wrote it**.

    The API is constructed here rather than passed in, so AC5's "the same prediction
    twice" is two independent Splink runs over one connection rather than one run
    read twice — and so every prediction in this module goes through `splink_api`,
    which is what the ticket's DoD claims about the whole repository.

    Read off the intermediate by its physical name rather than through
    `as_pandas_dataframe()`, because AC5 is a claim about row *order* and a
    round-trip through pandas is one more place the order could be imposed rather
    than observed.
    """
    linker = Linker(FRAME, settings=build_settings(cfg), db_api=splink_api(connection))
    predictions = linker.inference.predict(threshold_match_probability=REVIEW_LOW)
    scored = (
        f"SELECT record_key_l, record_key_r, match_probability FROM {predictions.physical_name}"
    )
    return [tuple(row) for row in connection.execute(scored).fetchall()]


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return load_config(Path(os.environ["ER_CONFIG"]))


@pytest.fixture
def teardown_witness(
    catalog: CatalogConnection, lake_conn: duckdb.DuckDBPyConnection
) -> Iterator[list[Any]]:
    """Assert AC6's *post*-teardown state, after `sub_namespace` has reclaimed.

    Requested **before** `sub_namespace` in the test signature, and that ordering is
    the whole mechanism: pytest finalises function-scoped fixtures in reverse setup
    order, so a fixture set up first is torn down last — which is the only vantage
    point from which "the namespace is gone" is observable at all. A test-body
    assertion or a `request.addfinalizer` both run *before* the fixture teardown they
    would be checking.
    """
    reclaimed: list[Any] = []
    yield reclaimed
    assert reclaimed, "the test registered no sub-namespace to check"
    for universe in reclaimed:
        assert not metadata_schema_exists(catalog, universe.namespace.metadata_schema), (
            f"{universe.namespace.metadata_schema} survived S8.1 step 4"
        )
    # The other half of AC6: the session namespace, which every other suite shares,
    # is the one a leak would actually contaminate.
    assert leaked_splink_relations(lake_conn) == ()


def test_scratch_schema_is_in_memory_not_lake(sub_namespace: Any) -> None:
    connection = sub_namespace("scratch").connection
    primary = scalar(connection, "SELECT current_database()")
    assert primary != LAKE_ALIAS, "S4.0b forbids DuckLake being the default catalog"

    splink_api(connection)

    # `output_schema` is not readable off the returned object; what it *does* is issue
    # `SET schema`, so the search path is the observable form of AC1's claim.
    assert scalar(connection, "SELECT current_schema()") == SPLINK_SCRATCH_SCHEMA
    schemas = {
        str(name)
        for (name,) in connection.execute(
            "SELECT database_name FROM duckdb_schemas() WHERE schema_name = ?",
            [SPLINK_SCRATCH_SCHEMA],
        ).fetchall()
    }
    assert schemas == {primary}, (
        f"{SPLINK_SCRATCH_SCHEMA} exists in {sorted(schemas)}; S4.0b puts it in the in-memory "
        f"primary database alone, because in the lake every intermediate would follow it"
    )


def test_prediction_leaves_zero_splink_relations_in_lake(sub_namespace: Any, cfg: Config) -> None:
    connection = sub_namespace("leak").connection
    primary = scalar(connection, "SELECT current_database()")
    materialize_frame(connection)

    pairs = predict_pairs(connection, cfg)

    # Both counts, per AC2: zero in the lake is only meaningful alongside evidence
    # that the run materialized anything at all.
    assert len(pairs) == PERSONA_COUNT, "the prediction scored nothing; the leak check is vacuous"
    assert splink_relation_count(connection, primary) > 0
    assert splink_relation_count(connection, LAKE_ALIAS) == 0
    assert_no_splink_relations_in_lake(connection)


def test_prediction_commits_no_lake_snapshot(sub_namespace: Any, cfg: Config) -> None:
    connection = sub_namespace("snapshot").connection
    before = current_snapshot(connection)

    materialize_frame(connection)
    assert predict_pairs(connection, cfg)

    # Not a count of snapshots -- S8.1 forbids that -- but the version itself, which
    # is unchanged exactly when Splink committed nothing.
    assert current_snapshot(connection) == before
    assert_no_splink_relations_in_lake(connection)


def test_assertion_fails_on_misconfigured_api(sub_namespace: Any, cfg: Config) -> None:
    connection = sub_namespace("misconfigured").connection
    primary = scalar(connection, "SELECT current_database()")
    try:
        # The misconstruction S4.0b forbids, both halves of it: DuckLake as the
        # default catalog, and no `output_schema` to send the intermediates anywhere
        # else. One `compute_tf_table` is enough -- it materializes a single
        # `__splink__df_tf_*` relation, so the arm proves the guard rather than
        # proving that DuckLake can survive a whole prediction.
        connection.execute(f"USE {LAKE_ALIAS}.main")
        materialize_frame(connection)
        api = DuckDBAPI(connection=connection)
        linker = Linker(FRAME, settings=build_settings(cfg), db_api=api)
        linker.table_management.compute_tf_table("email")

        leaked = leaked_splink_relations(connection)
        assert leaked, "the misconstructed API wrote nothing; the negative arm proves nothing"

        with pytest.raises(StageFailure) as refusal:
            assert_no_splink_relations_in_lake(connection)
        assert leaked[0] in str(refusal.value)
        assert refusal.value.code == 1
    finally:
        # Teardown calls `ducklake_expire_snapshots` on this connection; leaving the
        # lake as the default catalog would make that run against the wrong one.
        connection.execute(f"USE {primary}.main")


def test_threads_and_insertion_order_pinned(
    sub_namespace: Any, cfg: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    universe = sub_namespace("determinism")
    connection = universe.connection
    # A value the connection does not already carry, so the assertion cannot pass
    # against a connection that ignored the environment: `connect()` pinned whatever
    # Compose supplies, and `splink_api` must re-pin this.
    threads = int(os.environ["ER_DUCKDB_THREADS"]) + 1
    monkeypatch.setenv("ER_DUCKDB_THREADS", str(threads))

    splink_api(connection)

    assert scalar(connection, "SELECT current_setting('threads')") == threads
    assert scalar(connection, "SELECT current_setting('preserve_insertion_order')") is True

    materialize_frame(connection)
    first = predict_pairs(connection, cfg)
    second = predict_pairs(connection, cfg)

    assert first and first == second, "two runs of one prediction disagreed on order or probability"
    assert_no_splink_relations_in_lake(connection)


def test_runs_in_own_sub_namespace_and_tears_down(
    teardown_witness: list[Any],
    sub_namespace: Any,
    cfg: Config,
    er_env: Any,
) -> None:
    universe = sub_namespace("own")
    teardown_witness.append(universe)
    assert universe.namespace.metadata_schema != er_env.metadata_schema, (
        "the leak test shares the session namespace; a teardown failure could mask a leak"
    )

    connection = universe.connection
    materialize_frame(connection)
    assert predict_pairs(connection, cfg)
    assert leaked_splink_relations(connection) == ()
