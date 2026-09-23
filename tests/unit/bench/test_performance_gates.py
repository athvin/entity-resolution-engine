"""Quality budgets and deterministic preprocessing are observable contracts."""

import importlib
import json
import sys
from dataclasses import asdict, replace
from pathlib import Path

import duckdb
import pytest

from er.eval.metrics import pairwise_metrics_from_counts

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "benchmarks"))
sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "fixtures"))
_generator = importlib.import_module("generator.emit")
_personas = importlib.import_module("generator.personas")
CorpusSpec, emit_corpus = _generator.CorpusSpec, _generator.emit_corpus
generate_personas = _personas.generate_personas
acceptance = importlib.import_module("acceptance")
blocking = importlib.import_module("blocking_experiments")


@pytest.mark.parametrize(
    "missed,seconds,accepted", [(80, 80, True), (81, 80, False), (80, 81, False), (0, 99, True)]
)
def test_recall_budget_uses_percentage_points(missed: int, seconds: int, accepted: bool) -> None:
    baseline = pairwise_metrics_from_counts(790_000, 10_000, 10_000)
    candidate = pairwise_metrics_from_counts(790_000 - missed, 9_000, 10_000 + missed)
    verdict = acceptance.evaluate(
        baseline,
        candidate,
        baseline_seconds=100,
        candidate_seconds=seconds,
        records=10_000_000,
        repetitions=3,
    )
    assert verdict.accepted is accepted


def test_screening_and_precision_regressions_cannot_pass() -> None:
    baseline = pairwise_metrics_from_counts(100, 0, 1)
    candidate = pairwise_metrics_from_counts(100, 1, 1)
    result = acceptance.evaluate(
        baseline,
        candidate,
        baseline_seconds=100,
        candidate_seconds=50,
        records=1_000_000,
        repetitions=1,
    )
    assert not result.accepted and len(result.reasons) == 2


def test_campaign_rejects_initial_speedup_that_repeatedly_slows_incremental(tmp_path: Path) -> None:
    campaign = importlib.import_module("performance_campaign")
    quality = {"families": {"cluster": asdict(pairwise_metrics_from_counts(100, 1, 1))}}
    for arm, initial, incremental in [("current", 100, 10), ("em_1m", 60, 15)]:
        for iteration in range(1, 4):
            directory = tmp_path / f"{arm}-{iteration}"
            directory.mkdir()
            (directory / "trial.json").write_text(
                json.dumps(
                    {
                        "profile": "hard-v1",
                        "seed": 42,
                        "arm": arm,
                        "iteration": iteration,
                        "run": {
                            "processing_seconds": initial,
                            "incremental_seconds": incremental,
                            "correction_seconds": 20,
                            "initial_quality": quality,
                            "quality": quality,
                            "correction_quality": quality,
                        },
                    }
                )
            )
    comparison = next(
        row
        for row in campaign.report(tmp_path, "10m")["comparisons"]
        if row["workload"] == "initial"
    )
    assert comparison["repeatable_slowdowns"] == ["incremental"]
    assert all(not gate["accepted"] for gate in comparison["gates"])


def test_minhash_keys_are_pair_local_and_empty_strings_have_no_keys() -> None:
    with duckdb.connect() as c:
        c.execute(
            "CREATE TABLE inputs AS SELECT * FROM (VALUES "
            "('a','alice smith'),('b','alice smyth'),('c',''),('d','a')) t(input_id,text)"
        )
        keys = blocking.minhash_keys(c, "inputs", seed=123)
        before = c.execute(f"SELECT * FROM {keys} ORDER BY ALL").fetchall()
        assert len(before) == 2 * blocking.BANDS
        c.execute("INSERT INTO inputs VALUES ('e','alice smith'),('f','unrelated text')")
        blocking.minhash_keys(c, "inputs", seed=123)
        assert (
            c.execute(
                f"SELECT * FROM {keys} WHERE input_id IN ('a','b','c','d') ORDER BY ALL"
            ).fetchall()
            == before
        )
        assert c.execute(
            f"SELECT band,value FROM {keys} WHERE input_id='a' ORDER BY band"
        ).fetchall() == (
            c.execute(f"SELECT band,value FROM {keys} WHERE input_id='e' ORDER BY band").fetchall()
        )


def test_hard_profile_is_repeatable_and_keeps_truth_and_baseline_separate(tmp_path: Path) -> None:
    spec = CorpusSpec(seed=123, personas=500, records=1200, batch=50)
    personas = generate_personas(spec.seed, spec.personas, spec.household_rate)
    emit_corpus(spec, personas, tmp_path / "baseline")
    for name in ("hard1", "hard2"):
        emit_corpus(replace(spec, profile="hard-v1"), personas, tmp_path / name)
    for path in (tmp_path / "hard1").rglob("*.csv"):
        relative = path.relative_to(tmp_path / "hard1")
        assert path.read_bytes() == (tmp_path / "hard2" / relative).read_bytes()
        if path.name == "truth.csv":
            assert path.read_bytes() == (tmp_path / "baseline" / relative).read_bytes()
    assert (tmp_path / "hard1/crm.csv").read_bytes() != (tmp_path / "baseline/crm.csv").read_bytes()
    assert (tmp_path / "hard1/profile.json").is_file()


def test_sql_blocking_experiments_run_real_splink_without_lake_writes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from helpers.model import load_fixture_model

    from er.config.loader import load_config
    from er.lake.model import REGISTRY, create_table_sql

    monkeypatch.setenv("ER_DUCKDB_THREADS", "2")
    monkeypatch.setenv("ER_DUCKDB_MEMORY_LIMIT", "512MB")
    cfg = load_config(Path("configs/test.yaml"))
    (tmp_path / "truth.csv").write_text(
        "source_system,source_record_id,persona_id\ncrm,0,p\ncrm,1,p\ncrm,2,p\ncrm,3,p\n"
    )
    with duckdb.connect() as c:
        c.execute("ATTACH ':memory:' AS lake")
        for name in ("model_registry", "tf_lookup"):
            c.execute(create_table_sql(REGISTRY[name]))
        columns = ", ".join(
            column.sql.replace("LIST(VARCHAR)", "VARCHAR[]")
            for column in REGISTRY["int_std_records"].columns
        )
        c.execute(f"CREATE TABLE lake.main.int_std_records ({columns})")
        c.execute(
            "INSERT INTO lake.main.int_std_records (record_key,source_system,source_record_id,"
            "content_hash,std_version,name_variants,given_name,family_name,email,phone_e164,"
            "addr_postal,birth_date,ingest_batch_id,ingested_at) SELECT 'crm:' || i,"
            "'crm',i::VARCHAR,'hash','v1',['alice'],'alice','jones','alice@example.test',"
            "'+12125551234','10001', DATE '1980-01-01','batch',TIMESTAMP '2026-01-01' "
            "FROM range(4) t(i)"
        )
        c.execute(
            "CREATE TABLE lake.main.int_blocking_keys AS "
            + " UNION ALL ".join(
                f"SELECT record_key, '{rule.key_type}' key_type, ({rule.expr}) key_value "
                "FROM lake.main.int_std_records"
                for rule in cfg.blocking
            )
        )
        _, _, settings = load_fixture_model(c)
        monkeypatch.setattr(blocking, "load_model_settings", lambda *a: settings)
        monkeypatch.setattr(blocking.ObjectStore, "from_env", lambda: None)
        result = blocking.run_experiments(c, cfg, tmp_path, seed=123, include_batch=False)
        assert set(result["arms"]) == {
            "current",
            "without_name_postal",
            "minhash_plus",
            "minhash_replace",
        }
        assert all(arm["candidate_pairs"] == 6 for arm in result["arms"].values())
        assert all(
            arm["quality"]["families"]["cluster"]["recall"] == 1 for arm in result["arms"].values()
        )
        assert c.execute("SELECT count(*) FROM lake.main.int_blocking_keys").fetchone() == (16,)
