"""Bounded score consumption preserves thresholds, transactions and review payloads."""

import json
import math
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import duckdb
import pytest

from er.config.schema import Thresholds
from er.entities.ids import CountingIdFactory
from er.lake.bulk import BATCH_ROWS
from er.lake.model import REGISTRY, create_table_sql
from er.matching.full import (
    ScoredPair,
    _scored_rows,
    merge_match_scores,
    review_score_relation,
    review_scored_pairs,
)

THRESHOLDS = Thresholds(review_low=0.5, auto_merge=0.9)
EVIDENCE = {"gamma_email": 2, "bf_email": 41.7, "label": "O'Neil 雪"}


@pytest.mark.parametrize("native", [False, True])
@pytest.mark.parametrize(
    "evidence",
    [EVIDENCE, {"gamma_email": 2, "mw_email": 5.382, "mw_tf_adj_email": -0.7}],
    ids=["legacy-bayes-factors", "log2-weights"],
)
def test_score_batches_survive_review_writes_in_same_transaction(
    monkeypatch: pytest.MonkeyPatch,
    native: bool,
    evidence: dict[str, Any],
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
            [probabilities, json.dumps(evidence), count],
        )
        decode = json.loads
        decoded = 0

        def tracked_decode(value: str) -> Any:
            nonlocal decoded
            decoded += 1
            return decode(value)

        monkeypatch.setattr(json, "loads", tracked_decode)
        summary = (
            review_score_relation(
                connection,
                THRESHOLDS,
                model_version="v1",
                tf_snapshot_id="tf1",
                run_id="run1",
                id_factory=CountingIdFactory(),
            )
            if native
            else review_scored_pairs(
                connection,
                _scored_rows(connection, model_version="v1", tf_snapshot_id="tf1", run_id="run1"),
                THRESHOLDS,
                run_id="run1",
                id_factory=CountingIdFactory(),
            )
        )
        review_count = sum(i % 5 in (1, 2) for i in range(count))
        auto_count = sum(i % 5 in (3, 4) for i in range(count))
        assert summary.pairs_scored == count
        assert summary.pairs_above_auto_merge == auto_count
        assert summary.pairs_in_gray_band == review_count
        assert summary.review_queue_added == review_count
        assert summary.review_queue_refreshed == 0
        # Evidence of auto-merged and below-review pairs never becomes Python JSON.
        assert decoded == (0 if native else review_count)
        queued = connection.execute(
            "SELECT rec_a_key, match_probability, waterfall FROM lake.main.review_queue "
            "ORDER BY rec_a_key"
        ).fetchall()
        assert [(key, probability) for key, probability, _ in queued] == [
            (f"crm:{i:06}", probabilities[i % 5]) for i in range(count) if i % 5 in (1, 2)
        ]
        assert all(decode(payload) == evidence for _, _, payload in queued)
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


def test_merge_preserves_winning_evidence_and_hashes_from_a_view() -> None:
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(create_table_sql(REGISTRY["match_scores"]))
        connection.execute(
            "CREATE TABLE lake.main.int_std_records AS "
            "SELECT * FROM (VALUES ('crm:a', 'hash-a'), ('webforms:b', 'hash-b')) "
            "t(record_key, content_hash)"
        )
        connection.execute(
            "CREATE VIEW predictions AS SELECT * FROM (VALUES "
            "('crm:a', 'webforms:b', 0.7, '{\"selected\":false}'::JSON), "
            "('webforms:b', 'crm:a', 0.9, '{\"selected\":true}'::JSON), "
            "('crm:a', 'crm:a', 1.0, '{}'::JSON), "
            "('crm:a', 'missing:c', 1.0, '{}'::JSON)) "
            "t(record_key_l, record_key_r, match_probability, evidence)"
        )
        connection.execute("BEGIN")
        scored = list(
            merge_match_scores(
                connection,
                "predictions",
                "evidence",
                model_version="v1",
                tf_snapshot_id="tf1",
                run_id="run1",
            )
        )
        assert len(scored) == 1
        assert (scored[0].rec_a_key, scored[0].rec_b_key) == ("crm:a", "webforms:b")
        assert scored[0].match_probability == 0.9
        assert scored[0].evidence == {"selected": True}
        assert connection.execute(
            "SELECT rec_a_content_hash, rec_b_content_hash FROM lake.main.match_scores"
        ).fetchone() == ("hash-a", "hash-b")
        assert connection.execute(
            "SELECT count(*) FROM duckdb_tables() WHERE table_name LIKE 'er_match_stage_%'"
        ).fetchone() == (0,)
        connection.execute("ROLLBACK")
        assert connection.execute("SELECT count(*) FROM lake.main.match_scores").fetchone() == (0,)


def test_large_score_payloads_merge_and_stream_under_memory_limit(tmp_path: Path) -> None:
    """The old wide DISTINCT ON fails on this corpus at the same 512 MB limit."""
    with duckdb.connect(config={"threads": 2, "memory_limit": "2GB"}) as connection:
        connection.execute("SET temp_directory = ?", [str(tmp_path / "spill")])
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(create_table_sql(REGISTRY["match_scores"]))
        records = 1_000_000
        connection.execute(
            "CREATE TABLE lake.main.int_std_records AS SELECT p || i::VARCHAR record_key, "
            "md5(p || i::VARCHAR) content_hash FROM range(?) t(i) "
            "CROSS JOIN (VALUES ('crm:'), ('webforms:')) k(p)",
            [records],
        )
        connection.execute(
            "CREATE TABLE predictions AS SELECT 'crm:' || i::VARCHAR record_key_l, "
            "'webforms:' || i::VARCHAR record_key_r, 0.9::DOUBLE match_probability, "
            "json_object('payload', repeat(md5(i::VARCHAR), 16)) evidence FROM range(?) t(i)",
            [records],
        )
        connection.execute("SET memory_limit = '512MB'")
        scores = merge_match_scores(
            connection,
            "predictions",
            "evidence",
            model_version="v1",
            tf_snapshot_id="tf1",
            run_id="run1",
        )
        assert sum(1 for _ in scores) == records
        assert connection.execute("SELECT count(*) FROM lake.main.match_scores").fetchone() == (
            records,
        )
        assert connection.execute(
            "SELECT count(*) FROM duckdb_tables() WHERE table_name LIKE 'er_match_stage_%'"
        ).fetchone() == (0,)


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


@pytest.mark.parametrize("corruption", ["evidence", "duplicate_open"])
def test_native_review_keeps_legacy_error_and_committed_prefix(corruption: str) -> None:
    from er.errors import StageFailure

    observed = []
    for native in (False, True):
        with duckdb.connect() as connection:
            connection.execute("ATTACH ':memory:' AS lake")
            connection.execute(create_table_sql(REGISTRY["review_queue"]))
            connection.execute(
                "CREATE TABLE lake.main.match_scores AS "
                "SELECT 'crm:' || printf('%06d', i) rec_a_key, "
                "'web:' || printf('%06d', i) rec_b_key, 0.7::DOUBLE AS match_probability, "
                "?::JSON evidence, 'v1' model_version, 'tf1' tf_snapshot_id, 'run' run_id "
                "FROM range(?) t(i)",
                [json.dumps(EVIDENCE), 2 * BATCH_ROWS + 10],
            )
            bad = f"crm:{BATCH_ROWS + 3:06d}"
            if corruption == "evidence":
                connection.execute(
                    "UPDATE lake.main.match_scores SET evidence='{}' WHERE rec_a_key=?", [bad]
                )
            else:
                connection.execute(
                    "INSERT INTO lake.main.review_queue "
                    "SELECT 'existing' || i, 'pair', ?, ?, NULL, "
                    "'gray_band', 0.7, ?::JSON, 'open', 'before', 'before', NULL, NULL "
                    "FROM range(2) t(i)",
                    [bad, bad.replace("crm:", "web:"), json.dumps(EVIDENCE)],
                )
            with pytest.raises(StageFailure) as error:
                if native:
                    review_score_relation(
                        connection,
                        THRESHOLDS,
                        model_version="v1",
                        tf_snapshot_id="tf1",
                        run_id="run",
                        id_factory=CountingIdFactory(),
                    )
                else:
                    review_scored_pairs(
                        connection,
                        _scored_rows(
                            connection, model_version="v1", tf_snapshot_id="tf1", run_id="run"
                        ),
                        THRESHOLDS,
                        run_id="run",
                        id_factory=CountingIdFactory(),
                    )
            observed.append(
                (
                    str(error.value),
                    connection.execute(
                        "SELECT * FROM lake.main.review_queue ORDER BY review_id"
                    ).fetchall(),
                )
            )
    assert observed[0] == observed[1]
