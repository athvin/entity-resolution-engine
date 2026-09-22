"""Benchmark validation that keeps pair sets and sorting inside DuckDB."""

from __future__ import annotations

import errno
import hashlib
import json
import os
import shutil
import uuid
from dataclasses import asdict
from pathlib import Path
from typing import Any

from er.eval.metrics import pairwise_metrics_from_counts
from er.lake.bulk import staged_rows


def candidate_pair_count(connection: Any, *, partitions: int = 64) -> int:
    """Count each canonical pair once without one corpus-wide DISTINCT state.

    Numeric ranks preserve equality and record ordering. Partitioning by the
    smaller endpoint makes pair sets disjoint across passes, including pairs
    sharing several keys. Only the compact key table and one pass need memory.
    """
    if partitions < 1:
        raise ValueError("partitions must be positive")
    with staged_rows(
        connection,
        (("record_id", "BIGINT"), ("key_id", "BIGINT"), ("partition", "BIGINT")),
        (),
    ) as keys:
        connection.execute(
            f"INSERT INTO {keys} SELECT record_id, key_id, record_id % ? AS partition FROM ("
            "SELECT dense_rank() OVER (ORDER BY record_key) AS record_id, "
            "dense_rank() OVER (ORDER BY key_type, key_value) AS key_id "
            "FROM lake.main.int_blocking_keys WHERE record_key IS NOT NULL "
            "AND key_type IS NOT NULL AND key_value IS NOT NULL) ORDER BY partition",
            [partitions],
        )
        total = 0
        for partition in range(partitions):
            total += int(
                connection.execute(
                    "SELECT count(*) FROM (SELECT DISTINCT a.record_id, b.record_id "
                    f"FROM {keys} a JOIN {keys} b ON a.key_id=b.key_id "
                    "AND a.record_id < b.record_id WHERE a.partition=?)",
                    [partition],
                ).fetchone()[0]
            )
        return total


def file_sha256(path: Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def copy_input(source: Path, target: Path) -> None:
    """Link immutable benchmark inputs; copy when the filesystem cannot link them."""
    try:
        os.link(source, target)
    except OSError as error:
        if error.errno not in (errno.EXDEV, errno.EPERM, errno.EOPNOTSUPP):
            raise
        shutil.copy2(source, target)


def partition_sha256(connection: Any) -> str:
    """Hash the legacy sorted JSON partition without loading its groups into Python.

    Membership has one entity per record. Sorting groups by their smallest member
    therefore gives the same order as Python's lexicographic list sort.
    """
    cursor = connection.execute(
        "SELECT record_key, min(record_key) OVER (PARTITION BY entity_id) AS group_key "
        "FROM lake.main.entity_membership ORDER BY group_key, record_key"
    )
    digest = hashlib.sha256(b"[")
    previous = None
    for batch in iter(lambda: cursor.fetchmany(1024), []):
        for record_key, group_key in batch:
            if group_key != previous:
                digest.update(b"[" if previous is None else b"], [")
                previous = group_key
            else:
                digest.update(b", ")
            digest.update(json.dumps(str(record_key)).encode())
    digest.update(b"]" if previous is None else b"]]")
    return digest.hexdigest()


def _blocked_subset_counts(
    connection: Any, keys: str, query: str, parameters: list[Any] | None = None
) -> tuple[int, int]:
    """Restrict to supplied pairs before testing their shared blocking keys.

    Separate local stages prevent join reordering from expanding the entire
    blocking graph. Deduplication carries integer pair IDs, not string payloads.
    """
    with (
        staged_rows(
            connection,
            (("rec_a_key", "VARCHAR"), ("rec_b_key", "VARCHAR"), ("is_true", "BOOLEAN")),
            (),
        ) as pairs,
        staged_rows(
            connection,
            (("pair_id", "BIGINT"), ("rec_b_key", "VARCHAR"), ("key_id", "BIGINT")),
            (),
        ) as requested,
        staged_rows(connection, (("pair_id", "BIGINT"),), ()) as matched,
    ):
        connection.execute(f"INSERT INTO {pairs} {query}", parameters or [])
        connection.execute(
            f"INSERT INTO {requested} SELECT p.rowid, p.rec_b_key, k.key_id "
            f"FROM {pairs} p JOIN {keys} k ON p.rec_a_key=k.record_key"
        )
        connection.execute(
            f"INSERT INTO {matched} SELECT DISTINCT p.pair_id FROM {requested} p "
            f"JOIN {keys} k ON p.rec_b_key=k.record_key AND p.key_id=k.key_id"
        )
        total, true = connection.execute(
            "SELECT count(*), count(*) FILTER (WHERE p.is_true) "
            f"FROM {pairs} p JOIN {matched} m ON p.rowid=m.pair_id"
        ).fetchone()
        return int(total), int(true)


def quality_from_csv(
    connection: Any,
    corpus: Path,
    auto_merge: float,
    *,
    blocked_count: int,
    include_batch: bool = True,
) -> dict[str, Any]:
    """Count the three quality families over a fully labelled generated corpus.

    ``blocked_count`` is the distinct candidate count already measured by the
    benchmark. Truth and selected edges are joined to blocking keys in SQL;
    cluster closure sizes come from group counts rather than enumerated pairs.
    The common metric implementation owns all precision/recall conventions.
    ``include_batch=False`` measures the initial load before its delivery is ingested.
    """
    truth = f"temp.main.benchmark_truth_{uuid.uuid4().hex}"
    paths = [str(corpus / "truth.csv")]
    if include_batch and (corpus / "batch/truth.csv").exists():
        paths.append(str(corpus / "batch/truth.csv"))
    connection.execute(
        f"CREATE TEMP TABLE {truth} AS SELECT persona_id, "
        "source_system || ':' || source_record_id AS record_key "
        "FROM read_csv(?, header=true, all_varchar=true)",
        [paths],
    )

    def count(query: str, parameters: list[Any] | None = None) -> int:
        return int(connection.execute(query, parameters or []).fetchone()[0])

    def closure(groups: str) -> int:
        return count(f"SELECT coalesce(sum(n * (n - 1) // 2), 0) FROM ({groups})")

    try:
        invalid = connection.execute(
            f"SELECT record_key FROM {truth} GROUP BY record_key "
            "HAVING count(*) != 1 OR record_key IS NULL OR count(persona_id) != 1 LIMIT 1"
        ).fetchone()
        if invalid is not None:
            raise ValueError(f"invalid or duplicate truth record: {invalid[0]!r}")
        for table in ("int_std_records", "int_blocking_keys", "entity_membership"):
            stray = connection.execute(
                f"SELECT s.record_key FROM lake.main.{table} s "
                f"ANTI JOIN {truth} t USING (record_key) LIMIT 1"
            ).fetchone()
            if stray is not None:
                raise ValueError(f"{table} record {stray[0]!r} is outside the labelled corpus")
        stray = connection.execute(
            f"SELECT t.record_key FROM {truth} t "
            "ANTI JOIN lake.main.int_std_records s USING (record_key) LIMIT 1"
        ).fetchone()
        if stray is not None:
            raise ValueError(f"truth record {stray[0]!r} is outside the current corpus")

        truth_total = closure(f"SELECT count(*) n FROM {truth} GROUP BY persona_id")
        with staged_rows(connection, (("record_key", "VARCHAR"), ("key_id", "BIGINT")), ()) as keys:
            connection.execute(
                f"INSERT INTO {keys} SELECT record_key, "
                "dense_rank() OVER (ORDER BY key_type, key_value) "
                "FROM lake.main.int_blocking_keys WHERE record_key IS NOT NULL "
                "AND key_type IS NOT NULL AND key_value IS NOT NULL"
            )
            blocked_true, _ = _blocked_subset_counts(
                connection,
                keys,
                f"SELECT a.record_key, b.record_key, true FROM {truth} a "
                f"JOIN {truth} b ON a.persona_id=b.persona_id AND a.record_key < b.record_key",
            )
            edge_total, edge_true = _blocked_subset_counts(
                connection,
                keys,
                "WITH p AS (SELECT DISTINCT least(rec_a_key, rec_b_key) rec_a_key, "
                "greatest(rec_a_key, rec_b_key) rec_b_key FROM lake.main.match_scores "
                "WHERE is_active AND match_probability >= ?) "
                "SELECT p.rec_a_key, p.rec_b_key, a.persona_id=b.persona_id "
                f"FROM p JOIN {truth} a ON p.rec_a_key=a.record_key "
                f"JOIN {truth} b ON p.rec_b_key=b.record_key",
                [auto_merge],
            )
        cluster_total = closure(
            "SELECT count(DISTINCT record_key) n FROM lake.main.entity_membership "
            "GROUP BY entity_id"
        )
        cluster_true = closure(
            "SELECT count(DISTINCT m.record_key) n FROM lake.main.entity_membership m "
            f"JOIN {truth} t USING (record_key) GROUP BY m.entity_id, t.persona_id"
        )
        blocking = pairwise_metrics_from_counts(
            blocked_true, blocked_count - blocked_true, truth_total - blocked_true
        )
        edge = pairwise_metrics_from_counts(
            edge_true, edge_total - edge_true, blocked_true - edge_true
        )
        cluster = pairwise_metrics_from_counts(
            cluster_true, cluster_total - cluster_true, truth_total - cluster_true
        )
        return {
            "blocking_recall": blocking.recall,
            "quality": {
                "edge_precision": edge.precision,
                "edge_recall": edge.recall,
                "edge_f1": edge.f1,
                "cluster_precision": cluster.precision,
                "cluster_recall": cluster.recall,
                "cluster_f1": cluster.f1,
            },
            "families": {
                "blocking": asdict(blocking),
                "edge": asdict(edge),
                "cluster": asdict(cluster),
            },
        }
    finally:
        connection.execute(f"DROP TABLE {truth}")
