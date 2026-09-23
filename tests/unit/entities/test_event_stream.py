"""Paged event persistence retains exact hashes, reasons, IDs and dense sequences."""

import hashlib
import json
from datetime import datetime

import duckdb

from er.entities.ids import CountingIdFactory
from er.entities.relational import event_stream
from er.lake.bulk import BATCH_ROWS


def test_event_stream_preserves_bytes_and_sequence_across_pages() -> None:
    with duckdb.connect() as connection:
        connection.execute(
            "CREATE TABLE planned AS SELECT i+1 AS position, i::VARCHAR AS entity_id, "
            "'created' AS event_type, json_object('member_keys', ['web:雪', 'crm:O''Brien']) "
            "AS details FROM range(?) t(i)",
            [BATCH_ROWS + 3],
        )
        expected = json.dumps(
            {"member_keys": ["crm:O'Brien", "web:雪"], "reason": "correction_pass"},
            sort_keys=True,
            ensure_ascii=False,
            separators=(",", ":"),
        )
        expected_hash = hashlib.sha256(expected.encode()).hexdigest()
        for seq, event in enumerate(
            event_stream(
                connection,
                "planned",
                run_id="run",
                ids=CountingIdFactory(),
                reason="correction_pass",
            ),
            start=1,
        ):
            assert event.seq == seq
            row = event.row(datetime(2026, 1, 1))
            assert row[5] == expected
            assert row[6] == expected_hash
        assert seq == BATCH_ROWS + 3
