"""All three real Splink prediction paths use configuration over artifact rules."""

from datetime import datetime
from pathlib import Path

import duckdb
import pytest
from helpers.model import load_fixture_model

from er.config.loader import load_config
from er.lake.model import REGISTRY, create_table_sql
from er.matching.full import score_full
from er.matching.incremental import score_incremental
from er.obs.counters import StageCounters
from er.obs.runctx import StageRun


def test_stale_artifact_rules_do_not_suppress_full_or_either_incremental_pass(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("ER_DUCKDB_THREADS", "2")
    monkeypatch.setenv("ER_DUCKDB_MEMORY_LIMIT", "512MB")
    cfg = load_config(Path("configs/test.yaml"))
    with duckdb.connect() as c:
        c.execute("ATTACH ':memory:' AS lake")
        for name in (
            "match_scores",
            "review_queue",
            "model_registry",
            "tf_lookup",
            "ingest_batches",
            "runs",
            "run_stages",
        ):
            c.execute(create_table_sql(REGISTRY[name]))
        columns = ", ".join(
            column.sql.replace("LIST(VARCHAR)", "VARCHAR[]")
            for column in REGISTRY["int_std_records"].columns
        )
        c.execute(f"CREATE TABLE lake.main.int_std_records ({columns})")

        def records(start: int, end: int, batch: str) -> None:
            c.execute(
                "INSERT INTO lake.main.int_std_records (record_key,source_system,source_record_id,"
                "content_hash,std_version,name_variants,given_name,family_name,email,phone_e164,"
                "addr_postal,birth_date,ingest_batch_id,ingested_at) SELECT 'crm:' || i,"
                "'crm',i::VARCHAR,'hash','v1',['alice'],'alice','jones','alice@example.test',"
                "'+12125551234','10001', DATE '1980-01-01',?,TIMESTAMP '2026-01-01' "
                "FROM range(?,?) t(i)",
                [batch, start, end],
            )

        def stage(run: str) -> StageRun:
            return StageRun(
                run_id=run,
                stage="match",
                seq=1,
                started_at=datetime(2026, 1, 1),
                counters=StageCounters(()),
            )

        records(0, 2, "base")
        model, snapshot, settings = load_fixture_model(c)
        settings["blocking_rules_to_generate_predictions"] = ["1 = 0"]
        first = score_full(
            c, cfg, stage("full"), model_version=model, tf_snapshot_id=snapshot, settings=settings
        )
        assert first.pairs_scored == 1
        records(2, 4, "new")
        delta = score_incremental(
            c, cfg, stage("delta"), model_version=model, tf_snapshot_id=snapshot, settings=settings
        )
        assert delta.pairs_scored == 5  # Four new-old pairs and one new-new pair.
        assert settings["blocking_rules_to_generate_predictions"] == ["1 = 0"]
