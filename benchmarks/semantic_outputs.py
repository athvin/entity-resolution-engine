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
    pairs = connection.execute(
        "SELECT DISTINCT a.record_key, b.record_key FROM lake.main.int_blocking_keys a "
        "JOIN lake.main.int_blocking_keys b ON a.key_type=b.key_type AND a.key_value=b.key_value "
        "AND a.record_key < b.record_key ORDER BY 1,2"
    )
    digest = hashlib.sha256(b"[")
    separator = b""
    for page in iter(lambda: pairs.fetchmany(1024), []):
        for pair in page:
            digest.update(separator)
            digest.update(json.dumps(pair).encode())
            separator = b", "
    digest.update(b"]")
    hashes["candidate_pairs"] = digest.hexdigest()
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
