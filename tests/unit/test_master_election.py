"""Master-record election policies (er.golden.master).

The module renders a ``masters`` CTE per policy; these tests execute every
member of the vocabulary over one small fixture whose members are arranged so
that each policy elects a *different* winner — proof the ORDER BY fragments
are not accidentally equivalent — plus the deterministic ``record_key`` tie,
the unranked-source demotion, and the closed-vocabulary rejections.
"""

from __future__ import annotations

import duckdb
import pytest

from er.golden.master import (
    DEFAULT_MASTER_ELECTION_POLICY,
    MASTER_ELECTION_POLICIES,
    MasterElectionError,
    masters_cte_sql,
)
from er.lake.columns import GOLDEN_SURVIVABLE_COLUMNS

# Fixture shape (entity e1): the winner differs per policy.
#   billing:1 — updated 2026-06-01, 4 survivable attributes, 1 lineage win
#   crm:2     — updated 2026-01-01, 2 survivable attributes, 3 lineage wins
_RANKS_CRM_FIRST = {"crm": 1, "billing": 2}
_RANKS_BILLING_FIRST = {"billing": 1, "crm": 2}


@pytest.fixture
def lake() -> duckdb.DuckDBPyConnection:
    connection = duckdb.connect()
    survivable = ", ".join(f"{column} VARCHAR" for column in GOLDEN_SURVIVABLE_COLUMNS)
    connection.execute(
        f"CREATE TABLE int_std_records (record_key VARCHAR, {survivable}, "
        f"updated_at_source TIMESTAMP, ingested_at TIMESTAMP NOT NULL)"
    )
    connection.execute(
        "CREATE TABLE entity_membership (entity_id VARCHAR, record_key VARCHAR, "
        "source_system VARCHAR)"
    )
    connection.execute("CREATE TABLE golden_lineage (entity_id VARCHAR, record_key VARCHAR)")

    def std_row(record_key: str, filled: int, updated: str) -> None:
        values = [
            f"'v{index}'" if index < filled else "NULL"
            for index in range(len(GOLDEN_SURVIVABLE_COLUMNS))
        ]
        connection.execute(
            f"INSERT INTO int_std_records VALUES ('{record_key}', {', '.join(values)}, "
            f"TIMESTAMP '{updated}', TIMESTAMP '2026-01-01 00:00:00')"
        )

    std_row("billing:1", filled=4, updated="2026-06-01 00:00:00")
    std_row("crm:2", filled=2, updated="2026-01-01 00:00:00")
    connection.execute(
        "INSERT INTO entity_membership VALUES "
        "('e1', 'billing:1', 'billing'), ('e1', 'crm:2', 'crm')"
    )
    # crm:2 wins three golden attributes, billing:1 one.
    connection.execute(
        "INSERT INTO golden_lineage VALUES "
        "('e1', 'crm:2'), ('e1', 'crm:2'), ('e1', 'crm:2'), ('e1', 'billing:1')"
    )
    return connection


def elect(connection: duckdb.DuckDBPyConnection, policy: str, **kwargs: object) -> dict[str, str]:
    cte = masters_cte_sql(policy, schema_qualifier="main", **kwargs)  # type: ignore[arg-type]
    rows = connection.execute(
        f"WITH affected AS (SELECT DISTINCT entity_id FROM main.entity_membership), "
        f"masters AS ({cte}) SELECT entity_id, master_key FROM masters ORDER BY entity_id"
    ).fetchall()
    return dict(rows)


def test_each_policy_elects_its_own_winner(lake: duckdb.DuckDBPyConnection) -> None:
    assert elect(lake, "most_attributes") == {"e1": "crm:2"}
    assert elect(lake, "source_priority", source_ranks=_RANKS_BILLING_FIRST) == {"e1": "billing:1"}
    assert elect(lake, "source_priority", source_ranks=_RANKS_CRM_FIRST) == {"e1": "crm:2"}
    assert elect(lake, "most_recent") == {"e1": "billing:1"}
    assert elect(lake, "oldest") == {"e1": "crm:2"}
    assert elect(lake, "most_complete") == {"e1": "billing:1"}


def test_default_policy_is_the_shipped_lineage_election(
    lake: duckdb.DuckDBPyConnection,
) -> None:
    assert DEFAULT_MASTER_ELECTION_POLICY == "most_attributes"
    assert elect(lake, DEFAULT_MASTER_ELECTION_POLICY) == elect(lake, "most_attributes")


def test_ties_fall_to_the_lowest_record_key(lake: duckdb.DuckDBPyConnection) -> None:
    # Two members indistinguishable under every policy: same source, same
    # timestamps, same fill, same lineage contribution.
    lake.execute(
        "INSERT INTO entity_membership VALUES ('e2', 'crm:8', 'crm'), ('e2', 'crm:9', 'crm')"
    )
    lake.execute("INSERT INTO golden_lineage VALUES ('e2', 'crm:8'), ('e2', 'crm:9')")
    survivable = ", ".join("NULL" for _ in GOLDEN_SURVIVABLE_COLUMNS)
    for key in ("crm:8", "crm:9"):
        lake.execute(
            f"INSERT INTO int_std_records VALUES ('{key}', {survivable}, "
            f"NULL, TIMESTAMP '2026-01-01 00:00:00')"
        )
    for policy in MASTER_ELECTION_POLICIES:
        kwargs = {"source_ranks": _RANKS_CRM_FIRST} if policy == "source_priority" else {}
        assert elect(lake, policy, **kwargs)["e2"] == "crm:8", policy


def test_unranked_sources_are_demoted_not_elected(lake: duckdb.DuckDBPyConnection) -> None:
    # billing is absent from the mapping: crm wins even though billing:1 would
    # win under billing-first ranks.
    assert elect(lake, "source_priority", source_ranks={"crm": 5}) == {"e1": "crm:2"}


def test_vocabulary_is_closed(lake: duckdb.DuckDBPyConnection) -> None:
    with pytest.raises(MasterElectionError, match="not a master-election policy"):
        masters_cte_sql("newest", schema_qualifier="main")
    with pytest.raises(MasterElectionError, match="priority_rank"):
        masters_cte_sql("source_priority", schema_qualifier="main")
