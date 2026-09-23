"""Bounded canonical benchmark serialization; DuckDB owns joins and sorting."""

from __future__ import annotations

import gzip
import hashlib
import json
from collections.abc import Iterator
from pathlib import Path
from typing import Any

from er.lake.bulk import relation_pages, staged_query, staged_rows


def _identifier(name: str) -> str:
    return '"' + name.replace('"', '""') + '"'


def candidate_pair_sha256(connection: Any, *, partitions: int = 128) -> str:
    """Hash the sorted legacy JSON pair array without a corpus-wide DISTINCT.

    Ordered integer ranks preserve record-key ordering. Disjoint ranges of the
    smaller endpoint bound each deduplication/sort, including overlapping keys.
    Page encoding preserves json.dumps' exact bytes while avoiding one Python
    encoder call per pair. Neither the candidate definition nor its hash changes.
    """
    if partitions < 1:
        raise ValueError("partitions must be positive")
    digest = hashlib.sha256(b"[")
    separator = b""
    with staged_query(
        connection,
        "SELECT record_key, row_number() OVER (ORDER BY record_key) AS record_id "
        "FROM (SELECT DISTINCT record_key FROM lake.main.int_blocking_keys "
        "WHERE record_key IS NOT NULL AND key_type IS NOT NULL AND key_value IS NOT NULL)",
    ) as records:
        count = connection.execute(f"SELECT count(*) FROM {records}").fetchone()[0]
        if count:
            with staged_query(
                connection,
                "SELECT r.record_id, dense_rank() OVER (ORDER BY k.key_type, k.key_value) "
                f"AS key_id FROM lake.main.int_blocking_keys k JOIN {records} r "
                "USING(record_key) WHERE k.key_type IS NOT NULL AND k.key_value IS NOT NULL",
            ) as keys:
                width = (count + partitions - 1) // partitions
                for first in range(1, count + 1, width):
                    cursor = connection.execute(
                        "WITH pairs AS MATERIALIZED (SELECT DISTINCT a.record_id AS a_id, "
                        f"b.record_id AS b_id FROM {keys} a JOIN {keys} b "
                        "ON a.key_id=b.key_id AND a.record_id<b.record_id "
                        "WHERE a.record_id>=? AND a.record_id<?) "
                        f"SELECT l.record_key, r.record_key FROM pairs p JOIN {records} l "
                        f"ON p.a_id=l.record_id JOIN {records} r ON p.b_id=r.record_id "
                        "ORDER BY p.a_id, p.b_id",
                        [first, first + width],
                    )
                    for page in iter(lambda cursor=cursor: cursor.fetchmany(8192), []):
                        digest.update(separator)
                        digest.update(json.dumps(page).encode()[1:-1])
                        separator = b", "
    digest.update(b"]")
    return digest.hexdigest()


def save_semantic_outputs(
    connection: Any, config: Any, directory: Path, phase: str
) -> dict[str, str]:
    """Preserve existing JSON bytes/hashes without corpus-sized Python containers."""
    ignored = {
        "int_std_records": {"ingest_batch_id", "ingested_at"},
        "int_blocking_keys": set(),
        "golden_records": {"assembled_at"},
        "golden_lineage": {"assembled_at"},
    }
    hashes: dict[str, str] = {}
    with staged_query(
        connection,
        "SELECT entity_id, min(record_key) AS label FROM lake.main.entity_membership "
        "GROUP BY entity_id",
    ) as labels:
        for table, excluded in ignored.items():
            metadata = connection.execute(f"SELECT * FROM lake.main.{table} LIMIT 0")
            names = [column[0] for column in metadata.description if column[0] not in excluded]
            projection = ", ".join(
                "l.label AS entity_id" if name == "entity_id" else f"s.{_identifier(name)}"
                for name in names
            )
            join = f"LEFT JOIN {labels} l USING(entity_id)" if "entity_id" in names else ""
            with staged_query(
                connection,
                f"SELECT {projection}, row_number() OVER () AS position "
                f"FROM lake.main.{table} s {join}",
            ) as source:

                def encoded(names: list[str] = names) -> Iterator[tuple[str]]:
                    columns = ", ".join(_identifier(name) for name in names)
                    for page in relation_pages(connection, source, columns):
                        for values in page:
                            yield (
                                json.dumps(
                                    dict(zip(names, values, strict=True)),
                                    sort_keys=True,
                                    default=str,
                                    ensure_ascii=False,
                                ),
                            )

                # Python's exact JSON/float/date encoding is part of the comparison
                # format. Stage only encoded pages; DuckDB performs the global sort.
                with staged_rows(connection, (("encoded", "VARCHAR"),), encoded()) as serialized:
                    digest = hashlib.sha256()
                    separator = b""
                    cursor = connection.execute(
                        f"SELECT encoded FROM {serialized} ORDER BY encoded"
                    )
                    for page in iter(lambda cursor=cursor: cursor.fetchmany(1024), []):
                        for (row,) in page:
                            digest.update(separator)
                            digest.update(row.encode())
                            separator = b"\n"
                    hashes[table] = digest.hexdigest()
    hashes["candidate_pairs"] = candidate_pair_sha256(connection)
    scores = connection.execute(
        "SELECT rec_a_key, rec_b_key, match_probability, rec_a_content_hash, "
        "rec_b_content_hash, is_active, "
        "match_probability >= ? AND NOT isnan(match_probability), "
        "match_probability >= ? AND NOT isnan(match_probability) "
        "FROM lake.main.match_scores ORDER BY 1,2",
        [config.thresholds.auto_merge, config.thresholds.review_low],
    )
    digest = hashlib.sha256(b"[")
    separator_text = ""
    with gzip.open(directory / f"{phase}-scores.json.gz", "wt") as handle:
        handle.write("[")
        for page in iter(lambda: scores.fetchmany(1024), []):
            for row in page:
                handle.write(separator_text)
                handle.write(json.dumps(row[:6]))
                classified = (row[0], row[1], row[6], row[7], *row[3:6])
                digest.update(separator_text.encode())
                digest.update(json.dumps(classified).encode())
                separator_text = ", "
        handle.write("]")
    digest.update(b"]")
    hashes["score_classifications"] = digest.hexdigest()
    return hashes
