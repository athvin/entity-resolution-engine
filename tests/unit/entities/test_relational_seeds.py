"""Native seed discovery retains every delta arm and its watermark boundaries."""

from datetime import datetime

import duckdb
import pytest

from er.entities.cluster import seed_records
from er.entities.relational_stage import seed_query


@pytest.mark.parametrize("watermark", [None, datetime(2026, 1, 2), datetime(2026, 1, 4)])
def test_native_seed_union_matches_reference(watermark: datetime | None) -> None:
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(
            "CREATE TABLE lake.main.ingest_batches AS SELECT * FROM "
            "(VALUES ('current','run'), ('past','old')) t(ingest_batch_id,run_id)"
        )
        connection.execute(
            "CREATE TABLE lake.main.raw_records (source_system VARCHAR, source_record_id VARCHAR, "
            "content_hash VARCHAR, ingest_batch_id VARCHAR, is_deleted BOOLEAN, "
            "ingested_at TIMESTAMP)"
        )
        connection.execute(
            "INSERT INTO lake.main.raw_records VALUES "
            "('crm','batch','h','current',false,'2026-01-01'), "
            "('crm','changed','before','past',false,'2026-01-01'), "
            "('crm','changed','after','past',false,'2026-01-03'), "
            "('crm','boundary','before','past',false,'2026-01-01'), "
            "('crm','boundary','after','past',false,'2026-01-02'), "
            "('crm','deleted','zero','past',true,'2026-01-03'), "
            "('crm','revived','zero','past',true,'2026-01-01'), "
            "('crm','revived','after','past',false,'2026-01-03')"
        )
        connection.execute(
            "CREATE TABLE lake.main.assertions (rec_a_key VARCHAR, rec_b_key VARCHAR, "
            "created_at TIMESTAMP, retracted_at TIMESTAMP)"
        )
        connection.execute(
            "INSERT INTO lake.main.assertions VALUES "
            "('crm:a','crm:b','2026-01-03',NULL), "
            "('crm:c','crm:d','2026-01-01','2026-01-03'), "
            "('crm:e','crm:f','2026-01-02',NULL)"
        )
        connection.execute(
            "CREATE TABLE lake.main.review_queue (subject_type VARCHAR, status VARCHAR, "
            "rec_a_key VARCHAR, rec_b_key VARCHAR, entity_id VARCHAR, resolved_at TIMESTAMP)"
        )
        connection.execute(
            "INSERT INTO lake.main.review_queue VALUES "
            "('pair','dismissed','crm:review1','crm:review2',NULL,'2026-01-03'), "
            "('pair','open','crm:open1','crm:open2',NULL,NULL), "
            "('entity','resolved_match',NULL,NULL,'entity','2026-01-03'), "
            "('pair','resolved_no_match','crm:on1','crm:on2',NULL,'2026-01-02')"
        )
        connection.execute(
            "CREATE TABLE lake.main.entity_membership AS SELECT 'crm' source_system, "
            "'entitymember' source_record_id, 'crm:entitymember' record_key, 'entity' entity_id"
        )
        expected = seed_records(connection, run_id="run", watermark=watermark).records
        query, parameters = seed_query("run", watermark)
        assert {row[0] for row in connection.execute(query, parameters).fetchall()} == expected
