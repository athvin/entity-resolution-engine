"""Bounded score consumption preserves thresholds, transactions and review payloads."""

import json
import math
from collections.abc import Iterator
from typing import Any

import duckdb
import pytest

from er.config.schema import Thresholds
from er.entities.ids import CountingIdFactory
from er.lake.bulk import BATCH_ROWS
from er.lake.model import REGISTRY, create_table_sql
from er.matching.full import ScoredPair, _scored_rows, review_scored_pairs

THRESHOLDS = Thresholds(review_low=0.5, auto_merge=0.9)
EVIDENCE = {"gamma_email": 2, "bf_email": 41.7, "label": "O'Neil 雪"}


def test_native_score_batches_survive_review_writes_in_same_transaction(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(create_table_sql(REGISTRY["review_queue"]))
        connection.execute(
            "CREATE TABLE lake.main.match_scores (rec_a_key VARCHAR, rec_b_key VARCHAR, "
            "match_probability DOUBLE, evidence JSON, model_version VARCHAR, "
            "tf_snapshot_id VARCHAR, run_id VARCHAR)"
        )
        connection.execute("BEGIN")
        probabilities = [0.49, 0.5, math.nextafter(0.9, 0), 0.9, 1.0]
        count = 5 * BATCH_ROWS + 3
        connection.execute(
            "INSERT INTO lake.main.match_scores SELECT 'crm:' || printf('%06d', i), "
            "'webforms:' || printf('%06d', i), list_extract(?::DOUBLE[], (i % 5) + 1), "
            "?::JSON, 'v1', 'tf1', 'run1' FROM range(?) AS t(i)",
            [probabilities, json.dumps(EVIDENCE), count],
        )
        decode = json.loads
        decoded = 0

        def tracked_decode(value: str) -> Any:
            nonlocal decoded
            decoded += 1
            return decode(value)

        monkeypatch.setattr(json, "loads", tracked_decode)
        summary = review_scored_pairs(
            connection,
            _scored_rows(connection, model_version="v1", tf_snapshot_id="tf1", run_id="run1"),
            THRESHOLDS,
            run_id="run1",
            id_factory=CountingIdFactory(),
        )
        review_count = sum(i % 5 in (1, 2) for i in range(count))
        auto_count = sum(i % 5 in (3, 4) for i in range(count))
        assert summary.pairs_scored == count
        assert summary.pairs_above_auto_merge == auto_count
        assert summary.pairs_in_gray_band == review_count
        assert summary.review_queue_added == review_count
        assert summary.review_queue_refreshed == 0
        # Evidence of auto-merged and below-review pairs never becomes Python JSON.
        assert decoded == review_count
        queued = connection.execute(
            "SELECT rec_a_key, match_probability, waterfall FROM lake.main.review_queue "
            "ORDER BY rec_a_key"
        ).fetchall()
        assert [(key, probability) for key, probability, _ in queued] == [
            (f"crm:{i:06}", probabilities[i % 5]) for i in range(count) if i % 5 in (1, 2)
        ]
        assert all(decode(payload) == EVIDENCE for _, _, payload in queued)
        assert (
            review_scored_pairs(
                connection,
                _scored_rows(connection, model_version="v1", tf_snapshot_id="tf1", run_id="other"),
                THRESHOLDS,
                run_id="other",
            ).pairs_scored
            == 0
        )
        connection.execute("ROLLBACK")
        assert connection.execute("SELECT count(*) FROM lake.main.review_queue").fetchone() == (0,)
        assert connection.execute("SELECT count(*) FROM lake.main.match_scores").fetchone() == (0,)


def test_review_candidates_are_written_before_consuming_the_next_batch() -> None:
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(create_table_sql(REGISTRY["review_queue"]))
        count = 2 * BATCH_ROWS + 7

        def scores() -> Iterator[ScoredPair]:
            for index in range(count):
                if index and index % BATCH_ROWS == 0:
                    assert connection.execute(
                        "SELECT count(*) FROM lake.main.review_queue"
                    ).fetchone() == (index,)
                yield ScoredPair(
                    rec_a_key=f"crm:{index:06}",
                    rec_b_key=f"webforms:{index:06}",
                    match_probability=0.7,
                    evidence_json=json.dumps(EVIDENCE),
                )

        summary = review_scored_pairs(
            connection, scores(), THRESHOLDS, run_id="run1", id_factory=CountingIdFactory()
        )
        assert summary.pairs_scored == summary.review_queue_added == count
