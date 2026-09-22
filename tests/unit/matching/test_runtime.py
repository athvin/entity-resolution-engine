"""Execution controls reject invalid envelopes and preserve scoring across chunks."""

import json
import math
from datetime import date
from pathlib import Path

import duckdb
import pytest
from splink import Linker
from unit.matching.test_train_sequence import STD_RECORDS_DDL

from er.config.loader import load_config
from er.errors import ConfigError
from er.matching.api import cleanup_splink, splink_api
from er.matching.runtime import MatchingRuntime
from er.matching.tf import register_tf

ROOT = Path(__file__).resolve().parents[3]


@pytest.mark.parametrize(
    "env",
    [
        {"ER_SPLINK_NUM_CHUNKS_LEFT": "0"},
        {"ER_SPLINK_NUM_CHUNKS_RIGHT": "-1"},
        {"ER_SPLINK_NUM_CHUNKS_LEFT": "1.5"},
        {"ER_SPLINK_NUM_CHUNKS_LEFT": ""},
        {"ER_SPLINK_NUM_CHUNKS_RIGHT": "١"},
        {"ER_SPLINK_MATERIALISATION": "memory"},
        {"ER_SPLINK_MATERIALISATION": "parquet"},
        {"ER_SPLINK_MATERIALISATION": "parquet", "ER_SPLINK_WORK_DIR": "relative"},
        {"ER_SPLINK_MATERIALISATION": "parquet", "ER_SPLINK_WORK_DIR": "s3://lake/scratch"},
        {"ER_SPLINK_WORK_DIR": "/tmp/scratch"},
    ],
)
def test_invalid_runtime_is_rejected(env):
    with pytest.raises(ConfigError):
        MatchingRuntime.from_env(env)


@pytest.mark.parametrize("chunks", [1, 2, 4])
@pytest.mark.parametrize("materialisation", ["table", "parquet"])
def test_fixed_model_matches_splink4_and_cleans_scratch(
    monkeypatch, tmp_path, chunks, materialisation
):
    monkeypatch.setenv("ER_DUCKDB_THREADS", "2")
    monkeypatch.setenv("ER_SPLINK_MATERIALISATION", materialisation)
    monkeypatch.setenv("ER_SPLINK_NUM_CHUNKS_LEFT", str(chunks))
    monkeypatch.setenv("ER_SPLINK_NUM_CHUNKS_RIGHT", str(chunks))
    if materialisation == "parquet":
        monkeypatch.setenv("ER_SPLINK_WORK_DIR", str(tmp_path))
    else:
        monkeypatch.delenv("ER_SPLINK_WORK_DIR", raising=False)
    cfg = load_config(ROOT / "configs/test.yaml")
    settings = json.loads((ROOT / "tests/fixtures/splink4/model.json").read_text())
    oracle = json.loads((ROOT / "tests/fixtures/splink4/scores.json").read_text())["scores"]
    inputs = json.loads((ROOT / "tests/fixtures/splink4/inputs.json").read_text())
    records = [row[:6] + [date.fromisoformat(row[6]), row[7]] for row in inputs["records"]]
    settings["retain_intermediate_calculation_columns"] = True
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(STD_RECORDS_DDL)
        connection.executemany(
            "INSERT INTO lake.main.int_std_records VALUES (?, ?, ?, ?, ?, ?, ?, ?)", records
        )
        connection.execute(
            "CREATE TABLE lake.main.tf_lookup (model_version VARCHAR, tf_snapshot_id VARCHAR, "
            "column_name VARCHAR, value VARCHAR, tf_value DOUBLE)"
        )
        # Preserve model, records and TF independently of the new training algorithm.
        tf = inputs["tf"]
        connection.executemany("INSERT INTO lake.main.tf_lookup VALUES (?, ?, ?, ?, ?)", tf)
        api = splink_api(connection)
        connection.execute(
            "CREATE TABLE migration_corpus AS SELECT * FROM lake.main.int_std_records"
        )
        linker = Linker(api.register("migration_corpus"), settings=settings)
        register_tf(linker, connection, cfg, tf[0][0], tf[0][1])
        try:
            prediction = linker.inference.predict(
                threshold_match_probability=0.0,
                num_chunks_left=chunks,
                num_chunks_right=chunks,
            )
            cursor = connection.execute(
                f"SELECT * FROM {prediction.physical_name} ORDER BY record_key_l, record_key_r"
            )
            rows = [
                dict(zip([col[0] for col in cursor.description], row, strict=True))
                for row in cursor.fetchall()
            ]
            assert len(rows) == len(oracle)
            for actual, expected in zip(rows, oracle, strict=True):
                for name, value in expected.items():
                    if name == "match_probability":
                        assert actual[name] == pytest.approx(value, rel=0, abs=1e-10)
                        for threshold in (cfg.thresholds.review_low, cfg.thresholds.auto_merge):
                            assert (actual[name] >= threshold) == (value >= threshold)
                    elif name.startswith("bf_"):
                        assert actual[name.replace("bf_", "mw_", 1)] == pytest.approx(
                            math.log2(value), rel=0, abs=1e-10
                        )
                    else:
                        assert actual[name] == value
        finally:
            cleanup_splink(api)
        assert connection.execute("SELECT count(*) FROM migration_corpus").fetchone() == (24,)
        assert not list(tmp_path.rglob("*.parquet"))
        assert connection.execute(
            "SELECT count(*) FROM duckdb_tables() WHERE table_name LIKE '__splink__%'"
        ).fetchone() == (0,)


def test_training_failure_releases_parquet_scratch(monkeypatch, tmp_path):
    from unit.matching.test_train_sequence import corpus_rows

    from er.lake.ddl import create_statement
    from er.matching import train

    monkeypatch.setenv("ER_DUCKDB_THREADS", "1")
    monkeypatch.setenv("ER_SPLINK_MATERIALISATION", "parquet")
    monkeypatch.setenv("ER_SPLINK_WORK_DIR", str(tmp_path))
    cfg = load_config(ROOT / "configs/test.yaml")
    original = train._invoke

    def fail_after_em(linker, call):
        result = original(linker, call)
        if call.method_path.endswith("estimate_parameters_using_expectation_maximisation"):
            raise RuntimeError("injected failure after EM materialized scratch")
        return result

    monkeypatch.setattr(train, "_invoke", fail_after_em)
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(STD_RECORDS_DDL)
        connection.executemany(
            "INSERT INTO lake.main.int_std_records VALUES (?, ?, ?, ?, ?, ?, ?, ?)", corpus_rows()
        )
        connection.execute(create_statement("tf_lookup"))
        connection.execute("CREATE TABLE training_input AS SELECT * FROM lake.main.int_std_records")
        with pytest.raises(RuntimeError, match="injected failure"):
            train.train_model(
                connection, cfg, "training_input", model_version="v0001", tf_snapshot_id="test"
            )
        assert connection.execute("SELECT count(*) FROM training_input").fetchone() == (24,)
        assert not list(tmp_path.rglob("*.parquet"))
        assert connection.execute(
            "SELECT count(*) FROM duckdb_tables() WHERE table_name LIKE '__splink__%'"
        ).fetchone() == (0,)
