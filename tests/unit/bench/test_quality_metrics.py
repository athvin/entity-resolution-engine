"""Unit arm of the benchmark quality block (ER-100): the three S8.5 families, their
universes, delegation to the single `pairwise_metrics`, and the out-of-universe guard.

A tiny in-memory lake stands in for the substrate; `benchmarks/` is imported the way the
image imports it.
"""

from __future__ import annotations

import csv
import hashlib
import json
import random
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import duckdb
import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]


def _import(module: str) -> ModuleType:
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


quality = _import("quality")
large = _import("large_validation")


def test_quality_at_10k_does_not_construct_all_possible_pairs(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    connection = _lake()
    connection.execute(
        "INSERT INTO lake.main.int_std_records "
        "SELECT 'crm:' || i::VARCHAR FROM range(5, 10001) t(i)"
    )

    def forbidden(_records: object) -> None:
        raise AssertionError("quadratic record-pair universe was allocated")

    monkeypatch.setattr(quality, "_all_pairs", forbidden)
    try:
        assert quality.blocking_recall(connection, TRUTH).recall == 1
        assert quality.cluster_level_metrics(connection, TRUTH).f1 == 1
    finally:
        connection.close()


R1, R2, R3, R4 = "crm:1", "crm:2", "crm:3", "crm:4"
#: r1/r2 are one persona and r3/r4 another.
TRUTH = {(R1, R2), (R3, R4)}


def _lake() -> duckdb.DuckDBPyConnection:
    """Four records; blocking over-produces (1,3); edges and clusters link the true pairs."""
    conn = duckdb.connect(":memory:")
    conn.execute("ATTACH ':memory:' AS lake")
    conn.execute("CREATE TABLE lake.main.int_std_records (record_key VARCHAR)")
    conn.execute(
        "INSERT INTO lake.main.int_std_records VALUES ('crm:1'),('crm:2'),('crm:3'),('crm:4')"
    )
    conn.execute(
        "CREATE TABLE lake.main.int_blocking_keys "
        "(record_key VARCHAR, key_type VARCHAR, key_value VARCHAR)"
    )
    # Shared keys produce blocked pairs (1,2), (3,4) and the false candidate (1,3).
    conn.execute(
        "INSERT INTO lake.main.int_blocking_keys VALUES "
        "('crm:1','k','a'),('crm:2','k','a'),"  # (1,2)
        "('crm:3','k','b'),('crm:4','k','b'),"  # (3,4)
        "('crm:1','k','c'),('crm:3','k','c')"  # (1,3), a false candidate
    )
    conn.execute(
        "CREATE TABLE lake.main.match_scores "
        "(rec_a_key VARCHAR, rec_b_key VARCHAR, match_probability DOUBLE, is_active BOOLEAN)"
    )
    conn.execute(
        "INSERT INTO lake.main.match_scores VALUES "
        "('crm:1','crm:2',0.99,true),('crm:3','crm:4',0.98,true),('crm:1','crm:3',0.40,true)"
    )
    conn.execute("CREATE TABLE lake.main.entity_membership (record_key VARCHAR, entity_id VARCHAR)")
    conn.execute(
        "INSERT INTO lake.main.entity_membership VALUES "
        "('crm:1','E1'),('crm:2','E1'),('crm:3','E2'),('crm:4','E2')"
    )
    return conn


def test_three_families_and_universes() -> None:
    """AC4: blocking over full C(n,2); edge over the blocked set; cluster over C(n,2)."""
    conn = _lake()

    blocking = quality.blocking_recall(conn, TRUTH)
    # universe = C(4,2) = 6; blocked predicts (1,2),(1,3),(3,4); both true pairs are found.
    assert blocking.tp == 2 and blocking.fn == 0
    assert blocking.recall == 1.0
    assert blocking.fp == 1  # the (1,3) false candidate

    edge = quality.edge_level_metrics(conn, TRUTH, auto_merge=0.95)
    # universe = the blocked set (3 pairs); both true pairs are blocked and auto-merged.
    assert edge.tp == 2 and edge.fp == 0 and edge.fn == 0
    assert edge.precision == 1.0 and edge.recall == 1.0

    cluster = quality.cluster_level_metrics(conn, TRUTH)
    assert cluster.tp == 2 and cluster.fp == 0 and cluster.fn == 0
    assert cluster.precision == 1.0 and cluster.recall == 1.0

    block = quality.quality_block(conn, TRUTH, auto_merge=0.95)
    assert block["blocking_recall"] == 1.0
    assert set(block["quality"]) == {
        "edge_precision",
        "edge_recall",
        "edge_f1",
        "cluster_precision",
        "cluster_recall",
        "cluster_f1",
    }


def test_delegates_to_pairwise_metrics(monkeypatch: pytest.MonkeyPatch) -> None:
    """AC3: every number comes from `pairwise_metrics` — three calls, one per family."""
    conn = _lake()
    real = quality.pairwise_metrics
    calls: list[int] = []

    def spy(*args: Any, **kwargs: Any) -> Any:
        calls.append(1)
        return real(*args, **kwargs)

    monkeypatch.setattr(quality, "pairwise_metrics", spy)
    quality.quality_block(conn, TRUTH, auto_merge=0.95)
    assert len(calls) == 3, f"quality_block made {len(calls)} pairwise_metrics calls, expected 3"


def test_pair_outside_universe_raises() -> None:
    """AC6: a truth pair naming a record outside the universe raises, naming the pair."""
    conn = _lake()
    truth_with_ghost = TRUTH | {("crm:1", "crm:99")}  # crm:99 is not a current record
    with pytest.raises(ValueError) as raised:
        quality.cluster_level_metrics(conn, truth_with_ghost)
    assert "crm:99" in str(raised.value)


@pytest.mark.parametrize(
    "mutation",
    [
        "SELECT 1",
        "UPDATE lake.main.match_scores SET match_probability=1",
        "DELETE FROM lake.main.int_blocking_keys WHERE key_value='b'",
        "UPDATE lake.main.entity_membership SET entity_id='E1'",
        "UPDATE lake.main.entity_membership SET entity_id=record_key",
        "INSERT INTO lake.main.int_blocking_keys SELECT * FROM lake.main.int_blocking_keys",
        "INSERT INTO lake.main.match_scores SELECT * FROM lake.main.match_scores",
        "DELETE FROM lake.main.match_scores",
        "DELETE FROM lake.main.int_blocking_keys",
    ],
)
def test_sql_quality_matches_pair_sets(tmp_path: Path, mutation: str) -> None:
    with _lake() as connection:
        connection.execute(mutation)
        # Include a batch sidecar to exercise combined initial/incremental truth.
        (tmp_path / "batch").mkdir()
        for directory, rows in (
            (tmp_path, [("p1", "crm", "1"), ("p1", "crm", "2")]),
            (tmp_path / "batch", [("p2", "crm", "3"), ("p2", "crm", "4")]),
        ):
            with (directory / "truth.csv").open("w") as handle:
                writer = csv.writer(handle)
                writer.writerow(["persona_id", "source_system", "source_record_id"])
                writer.writerows(rows)
        expected = quality.quality_block(connection, TRUTH, auto_merge=0.95)
        assert (
            large.quality_from_csv(
                connection, tmp_path, 0.95, blocked_count=len(quality._blocked_pairs(connection))
            )
            == expected
        )
        assert not connection.execute(
            "SELECT table_name FROM duckdb_tables() WHERE table_name LIKE 'benchmark_truth_%'"
        ).fetchall()


def test_initial_quality_excludes_a_future_delivery(tmp_path: Path) -> None:
    (tmp_path / "truth.csv").write_text(
        "persona_id,source_system,source_record_id\np1,crm,1\np1,crm,2\np2,crm,3\np2,crm,4\n"
    )
    (tmp_path / "batch").mkdir()
    (tmp_path / "batch/truth.csv").write_text(
        "persona_id,source_system,source_record_id\np3,crm,5\n"
    )
    with _lake() as connection:
        expected = quality.quality_block(connection, TRUTH, auto_merge=0.95)
        assert (
            large.quality_from_csv(connection, tmp_path, 0.95, blocked_count=3, include_batch=False)
            == expected
        )
        with pytest.raises(ValueError, match="outside the current corpus"):
            large.quality_from_csv(connection, tmp_path, 0.95, blocked_count=3)


@pytest.mark.parametrize("records", [0, 5, 2500])
def test_streamed_partition_preserves_legacy_hash(records: int) -> None:
    rows = [(f'crm:{index:05d}é"\n', str(index % 3)) for index in range(records)]
    random.Random(42).shuffle(rows)
    groups: dict[str, list[str]] = {}
    for key, entity in rows:
        groups.setdefault(entity, []).append(key)
    expected = hashlib.sha256(
        json.dumps(sorted(sorted(group) for group in groups.values())).encode()
    ).hexdigest()
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(
            "CREATE TABLE lake.main.entity_membership (record_key VARCHAR, entity_id VARCHAR)"
        )
        if rows:
            connection.executemany("INSERT INTO lake.main.entity_membership VALUES (?, ?)", rows)
        assert large.partition_sha256(connection) == expected


@pytest.mark.parametrize("defect", ["missing", "extra", "duplicate"])
def test_sql_quality_rejects_incomplete_truth(tmp_path: Path, defect: str) -> None:
    rows = "p1,crm,1\np1,crm,2\np2,crm,3\np2,crm,4\n"
    if defect == "missing":
        rows = rows.replace("p2,crm,4\n", "")
    else:
        rows += "p2,crm,4\n" if defect == "duplicate" else "p2,crm,99\n"
    (tmp_path / "truth.csv").write_text("persona_id,source_system,source_record_id\n" + rows)
    with _lake() as connection, pytest.raises(ValueError, match="truth|corpus"):
        large.quality_from_csv(connection, tmp_path, 0.95, blocked_count=3)


@pytest.mark.parametrize("partitions", [1, 3, 64])
def test_partitioned_candidate_count_matches_exact_pair_set(partitions: int) -> None:
    randomizer = random.Random(42)
    rows = [
        (
            f"crm:{randomizer.randrange(40)}",
            randomizer.choice(["email", "name", None]),
            randomizer.choice(["x", "y", "O'Brien", None]),
        )
        for _ in range(300)
    ]
    rows += rows[:20]
    expected = {
        (a, b)
        for a, ta, va in rows
        for b, tb, vb in rows
        if a < b and ta is not None and va is not None and ta == tb and va == vb
    }
    with _lake() as connection:
        connection.execute("DELETE FROM lake.main.int_blocking_keys")
        connection.executemany("INSERT INTO lake.main.int_blocking_keys VALUES (?, ?, ?)", rows)
        assert large.candidate_pair_count(connection, partitions=partitions) == len(expected)
        assert not connection.execute(
            "SELECT table_name FROM duckdb_tables() WHERE table_name LIKE 'er_bulk_%'"
        ).fetchall()
        connection.execute("DELETE FROM lake.main.int_blocking_keys")
        assert large.candidate_pair_count(connection, partitions=partitions) == 0
