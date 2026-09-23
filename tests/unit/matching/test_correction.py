"""Correction TF identity survives failure, retries and corpus drift."""

from datetime import datetime
from pathlib import Path

import duckdb
import pytest

from er.config.loader import load_config
from er.errors import PreconditionFailure
from er.lake.model import REGISTRY, create_table_sql
from er.matching.correction import assert_no_pending_correction, prepare_correction
from er.matching.tf import tf_columns
from er.obs.counters import StageCounters
from er.obs.runctx import StageRun
from er.resume import read_resume_rows, resume_plan


def test_correction_freezes_once_and_blocks_other_writers() -> None:
    cfg = load_config(Path("configs/test.yaml"))
    with duckdb.connect() as c:
        c.execute("ATTACH ':memory:' AS lake")
        c.execute(create_table_sql(REGISTRY["tf_lookup"]))
        c.execute(
            "CREATE TABLE lake.main.runs (run_id VARCHAR, mode VARCHAR, "
            "status VARCHAR, started_at TIMESTAMP, model_version VARCHAR, "
            "tf_snapshot_id VARCHAR, config_hash VARCHAR)"
        )
        c.execute(
            "INSERT INTO lake.main.runs VALUES "
            "('run','correction_pass','failed','2026-01-01','v1',NULL,'hash')"
        )
        c.execute(
            "CREATE TABLE lake.main.run_stages (run_id VARCHAR, stage VARCHAR, "
            "seq INTEGER, status VARCHAR)"
        )
        columns = ", ".join(f"'value'::VARCHAR AS {column}" for column in tf_columns(cfg))
        c.execute(f"CREATE TABLE lake.main.int_std_records AS SELECT {columns}")
        stage = StageRun(
            run_id="run",
            stage="match",
            seq=1,
            started_at=datetime(2026, 1, 1),
            counters=StageCounters(()),
        )
        snapshot = prepare_correction(c, cfg, stage, "v1")
        frozen = c.execute("SELECT * FROM lake.main.tf_lookup ORDER BY ALL").fetchall()
        assert frozen and stage.tf_snapshot_id == snapshot
        c.execute("UPDATE lake.main.int_std_records SET given_name='changed'")
        assert prepare_correction(c, cfg, stage, "v1") == snapshot
        assert c.execute("SELECT * FROM lake.main.tf_lookup ORDER BY ALL").fetchall() == frozen
        with pytest.raises(PreconditionFailure, match="er correct --resume run"):
            assert_no_pending_correction(c)
        assert_no_pending_correction(c, resume_run_id="run")
        assert_no_pending_correction(c, assertion_repair=True)
        with pytest.raises(PreconditionFailure, match="MODEL_VERSION_CHANGED"):
            prepare_correction(c, cfg, stage, "v2")
        # No stage yet, crash between stages, and crash after the final commit.
        assert resume_plan(read_resume_rows(c, "run"), "hash").resume_from == "match"
        for seq, name, next_stage in [
            (1, "match", "reconcile"),
            (2, "reconcile", "assemble"),
            (3, "assemble", "assemble"),
        ]:
            c.execute(
                "INSERT INTO lake.main.run_stages VALUES ('run',?,?, 'succeeded')", [name, seq]
            )
            assert resume_plan(read_resume_rows(c, "run"), "hash").resume_from == next_stage
            if name == "match":
                assert_no_pending_correction(c, assertion_repair=True)
            else:
                with pytest.raises(PreconditionFailure, match="er correct --resume run"):
                    assert_no_pending_correction(c, assertion_repair=True)
        c.execute("UPDATE lake.main.runs SET status='succeeded'")
        assert_no_pending_correction(c)
        assert_no_pending_correction(c, assertion_repair=True)
        with pytest.raises(PreconditionFailure, match="already succeeded"):
            resume_plan(read_resume_rows(c, "run"), "hash")
