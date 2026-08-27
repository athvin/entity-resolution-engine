"""S4.5.5's retraction path: an edge scored against a version that no longer exists.

`raw_records` is append-only (S4.1), so re-delivering a record with corrected values
does not overwrite anything — it adds a version, and `int_std_records` then carries the
*current* one. Every `match_scores` row scored before that delivery was scored against
the old version, and INV-SCORE (S4.3.3) says a probability is a pure function of
`(model_version, tf_snapshot_id, rec_a_key, rec_b_key, rec_a_content_hash,
rec_b_content_hash)` — so those rows are not wrong, they are **about a pair that no
longer exists**. Leaving them active would cluster the corpus on evidence about
superseded values.

:func:`invalidate_incident_edges` is the one statement that retires them. Three
properties of it are load-bearing:

* **It is an `UPDATE`, never a `DELETE` and never an `INSERT`.** `match_scores`' logical
  key is `(rec_a_key, rec_b_key, model_version, tf_snapshot_id)` and S5.0 permits **at
  most one row** for it regardless of `is_active`. Invalidating by inserting a
  superseding row would double every logical key; deleting would destroy the record of
  what was once believed, which is what `invalidated_at` exists to keep.
* **It runs BEFORE scoring, in the same run, and from the STAGE rather than the
  scorer.** So a pair whose endpoints still match is invalidated and then immediately
  re-scored and overwritten back to `is_active = true` by the S4.3.4 `MERGE`. That is
  INV-SCORE working rather than breaking: the row ends up carrying the *new* endpoint
  hashes and a probability derived from them. The call site is
  `er.cli._MatchStage.run`, one level out from :func:`~er.matching.full.score_full` and
  :func:`~er.matching.incremental.score_incremental`, because S4.0b permits the scoring
  path exactly one write to `match_scores` — "only final scored pairs are written ... in
  a single write statement" — and a retirement is not a scored pair. Putting it inside
  either scorer makes that stage write twice, which
  `tests/integration/test_full_match.py::test_single_write_statement_to_match_scores`
  exists to refuse.
* **An absent endpoint counts as stale.** S4.2 excludes a tombstoned record from the
  standardized corpus entirely, so "no `int_std_records` row" is how a deletion presents
  itself here. The comparison is therefore a `LEFT JOIN` with a NULL test rather than an
  equality that would silently keep the edge alive (S4.5.5).

The deletion arm proper — tombstone derivation and the `--full-refresh-keys` path — is
ER-032's and ER-083's. This module only honours what it finds in `int_std_records`.
"""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Final

import duckdb

from er.lake.model import SCHEMA_QUALIFIER

__all__ = [
    "MATCH_SCORES_RELATION",
    "StaleEdge",
    "invalidate_incident_edges",
    "stale_edge_rows",
]

MATCH_SCORES_RELATION: Final = "match_scores"
_MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.{MATCH_SCORES_RELATION}"
_STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"

#: One stale edge: the pair, and which endpoint went stale. Returned rather than merely
#: counted because "three edges were invalidated" is a number, and the operator's next
#: question is always which record moved underneath them.
StaleEdge = tuple[str, str, str]

#: The predicate, written once and shared by the read and the write so the two cannot
#: disagree about what "stale" means. An endpoint is stale when the corpus no longer
#: holds it (S4.2 excludes a tombstoned record) or holds it under a different
#: `content_hash` than the row was scored against (S4.3.3).
_STALE_PREDICATE: Final = """
      m.is_active
  AND (
        a.record_key IS NULL
     OR b.record_key IS NULL
     OR a.content_hash <> m.rec_a_content_hash
     OR b.content_hash <> m.rec_b_content_hash
  )
"""

_JOIN: Final = f"""
  FROM {_MATCH_SCORES} AS m
  LEFT JOIN {_STD_RECORDS} AS a ON a.record_key = m.rec_a_key
  LEFT JOIN {_STD_RECORDS} AS b ON b.record_key = m.rec_b_key
"""


def stale_edge_rows(connection: duckdb.DuckDBPyConnection) -> list[StaleEdge]:
    """Every active edge scored against an endpoint version the corpus no longer holds.

    The read half of :func:`invalidate_incident_edges`, sharing its predicate. Exposed
    so a caller — a test, or an operator asking why a cluster changed — can name the
    edges before or after they are retired, rather than inferring them from a count.

    Args:
        connection: a connection with the lake attached (S4.0b). Nothing is written.

    Returns:
        `(rec_a_key, rec_b_key, reason)` per stale edge, in canonical pair order, where
        `reason` names which endpoint went stale.
    """
    rows = connection.execute(
        f"""
        SELECT m.rec_a_key, m.rec_b_key,
               CASE
                 WHEN a.record_key IS NULL AND b.record_key IS NULL THEN 'both endpoints absent'
                 WHEN a.record_key IS NULL THEN 'rec_a absent'
                 WHEN b.record_key IS NULL THEN 'rec_b absent'
                 WHEN a.content_hash <> m.rec_a_content_hash
                  AND b.content_hash <> m.rec_b_content_hash THEN 'both hashes changed'
                 WHEN a.content_hash <> m.rec_a_content_hash THEN 'rec_a hash changed'
                 ELSE 'rec_b hash changed'
               END AS reason
        {_JOIN}
         WHERE {_STALE_PREDICATE}
         ORDER BY m.rec_a_key, m.rec_b_key
        """
    ).fetchall()
    return [(str(rec_a), str(rec_b), str(reason)) for rec_a, rec_b, reason in rows]


def invalidate_incident_edges(
    connection: duckdb.DuckDBPyConnection,
    *,
    run_id: str,
    now: datetime | None = None,
) -> int:
    """Retire every active edge whose endpoints have moved. Returns rows updated.

    One statement, so the retirement is one DuckLake snapshot and no reader can observe
    half of it. The correlated subqueries repeat the join rather than sharing the CTE
    the read uses, because DuckDB's `UPDATE` takes no `FROM` clause and hoisting the
    predicate into a temporary relation would make this two statements — which is
    exactly the atomicity this function exists to provide.

    Args:
        connection: a connection with the lake attached (S4.0b).
        run_id: this run, written to `invalidated_run_id` so the retirement is traceable
            to the delivery that caused it.
        now: the stamp for `invalidated_at`; now, in UTC, when omitted.

    Returns:
        How many rows were invalidated. Zero is the ordinary case: a run in which no
        record's `content_hash` moved retires nothing, which is what AC4 asserts.
    """
    stamp = datetime.now(UTC).replace(tzinfo=None) if now is None else now
    before = _active_count(connection)
    connection.execute(
        f"""
        UPDATE {_MATCH_SCORES} AS m
           SET is_active = false,
               invalidated_at = ?,
               invalidated_run_id = ?
         WHERE m.is_active
           AND NOT EXISTS (
                 SELECT 1
                   FROM {_STD_RECORDS} AS a
                   JOIN {_STD_RECORDS} AS b ON b.record_key = m.rec_b_key
                  WHERE a.record_key = m.rec_a_key
                    AND a.content_hash = m.rec_a_content_hash
                    AND b.content_hash = m.rec_b_content_hash
               )
        """,
        [stamp, run_id],
    )
    return before - _active_count(connection)


def _active_count(connection: duckdb.DuckDBPyConnection) -> int:
    """How many `match_scores` rows are currently active."""
    row = connection.execute(f"SELECT count(*) FROM {_MATCH_SCORES} WHERE is_active").fetchone()
    return 0 if row is None else int(row[0])
