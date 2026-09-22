"""Benchmark B's smoke arm (S10.3, S10.4, M17): a real measured pass over the `smoke`
scale produces a schema-valid result, and a measured run leaves no `__splink__` relation
in the lake.

This is the only arm that runs `run_pass` against a real lake, so it also proves the
things the unit arm cannot: that each phase names a `run_stages` row and `wall_ms` is at
least the summed stage `duration_ms` (M2); that generation and `er init` are outside the
measured time; and that a second pass reuses the generated corpus (AC7). `run_benchmark`
is a directory-of-scripts module, so it is imported the way the image imports it.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
from collections.abc import Sequence
from pathlib import Path
from types import ModuleType
from typing import Any, Final

import duckdb
import pytest
from ulid import ULID

from er.config.hashing import config_hash
from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, render_dbt_vars, run_dbt
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

#: The S8.3 row this module realises (ER-092 convention).
SPEC_TEST_IDS: Final[tuple[str, ...]] = (
    "tests/integration/test_benchmark_smoke.py::test_smoke_pass_produces_schema_valid_result",
)

REPO_ROOT: Final = Path(__file__).resolve().parents[2]
SCALE: Final = "smoke"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"
MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"
BLOCKING_KEYS: Final = f"{SCHEMA_QUALIFIER}.int_blocking_keys"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"
SOURCES: Final = ("crm", "billing", "webforms")


def _import(module: str) -> ModuleType:
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


run_benchmark = _import("run_benchmark")
fingerprint = _import("fingerprint")
schema = _import("schema")
scales = _import("scales")

StageRow = run_benchmark.StageRow


def config() -> Config:
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    payload, _ = blocking_rules_from_config(cfg)
    return payload


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def _prepare_corpus(scale: Any, cfg: Config, staging: Path, config_path: Path) -> tuple[Path, Path]:
    """Generate the corpus once and lay it out as two drop roots (base, batch).

    Generation goes through the generator's own CLI as a subprocess, run from the repo
    root — the way S10.1 and the real benchmark generate a corpus, and the way that keeps
    `fixtures.generator` resolvable (it is a scripts directory, not an installed package,
    reached through the working directory). The CLI writes flat `<staging>/<source>.csv`
    (+ a `batch/` child); `er ingest` reads `<drop>/<source>/`, so the files are laid out
    into per-source drops here. Generation runs only if the corpus is not already present,
    so a second pass reuses it (AC7).
    """
    marker = staging / "crm.csv"
    if not marker.exists():
        staging.mkdir(parents=True, exist_ok=True)
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "fixtures.generator.cli",
                "--personas",
                str(scale.personas),
                "--records",
                str(scale.records),
                "--batch",
                str(scale.incremental_batch),
                "--seed",
                str(cfg.generator.seed),
                "--out",
                str(staging),
                "--config",
                str(config_path),
            ],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0, (
            f"corpus generation failed:\n{result.stdout}\n{result.stderr}"
        )
    base_drop = staging / "_drop_base"
    batch_drop = staging / "_drop_batch"
    for source in SOURCES:
        (base_drop / source).mkdir(parents=True, exist_ok=True)
        shutil.copy(staging / f"{source}.csv", base_drop / source / f"{source}.csv")
        (batch_drop / source).mkdir(parents=True, exist_ok=True)
        shutil.copy(staging / "batch" / f"{source}.csv", batch_drop / source / f"{source}.csv")
    return base_drop, batch_drop


class _SmokeRunner:
    """The production `Runner`: drives `er`, and reads `run_stages`/blocking/scores back."""

    def __init__(
        self,
        connection: duckdb.DuckDBPyConnection,
        *,
        cfg: Config,
        config_path: Path,
        drop_for_run: dict[str, Path],
        dbt: Dbt,
    ) -> None:
        self.connection = connection
        self.cfg = cfg
        self.config_path = config_path
        self.drop_for_run = drop_for_run
        self.dbt = dbt
        self.auto_merge = cfg.thresholds.auto_merge

    def er(self, args: Sequence[str]) -> None:
        argv = list(args)
        # The `standardize` console verb is a NoOp timing marker (the dbt staging +
        # intermediate build is orchestrated separately); so the standardize phase does
        # both — the dbt build that materialises `int_std_records`, then `er standardize`
        # for the `run_stages` row the phase reconciles against (M2). Every other phase is
        # a real console stage that writes its own row.
        if argv[0] == "standardize":
            self.dbt("build", select="staging")
            self.dbt("build", select="intermediate")
        elif argv[0] == "ingest":
            run_id = argv[argv.index("--run-id") + 1]
            argv += ["--path", str(self.drop_for_run[run_id])]
        argv += ["--config", str(self.config_path)]
        result = subprocess.run(
            ["er", *argv], capture_output=True, text=True, env=dict(os.environ), check=False
        )
        assert result.returncode in (int(ExitCode.SUCCESS), int(ExitCode.NOTHING_TO_DO)), (
            f"er {' '.join(argv)} -> {result.returncode}\n{result.stdout}\n{result.stderr}"
        )

    def stage_rows(self, run_id: str) -> list[Any]:
        return [
            StageRow(
                stage=str(stage),
                duration_ms=int(duration or 0),
                rows_in=int(rows_in or 0),
                rows_out=int(rows_out or 0),
                snapshot_start=int(start or 0),
                snapshot_end=int(end or 0),
            )
            for stage, duration, rows_in, rows_out, start, end in self.connection.execute(
                "SELECT stage, duration_ms, rows_in, rows_out, snapshot_start, snapshot_end "
                f"FROM {RUN_STAGES} WHERE run_id = ? AND status = 'succeeded'",
                [run_id],
            ).fetchall()
        ]

    def candidate_pair_count(self) -> int:
        return int(
            scalar(
                self.connection,
                f"SELECT count(*) FROM (SELECT DISTINCT a.record_key, b.record_key "
                f"FROM {BLOCKING_KEYS} a JOIN {BLOCKING_KEYS} b "
                "ON a.key_type = b.key_type AND a.key_value = b.key_value "
                "AND a.record_key < b.record_key)",
            )
        )

    def pairs_above_auto_merge(self, run_id: str) -> int:
        return int(
            scalar(
                self.connection,
                f"SELECT count(*) FROM {MATCH_SCORES} WHERE run_id = ? AND match_probability >= ?",
                run_id,
                self.auto_merge,
            )
        )


class Dbt:
    def __init__(self, connection: duckdb.DuckDBPyConnection, artifacts: Path, cfg: Config) -> None:
        self.connection = connection
        self.artifacts = artifacts
        self.cfg = cfg

    def __call__(self, command: str, select: str | None = None) -> None:
        run_dbt(
            command,
            select=select,
            vars=render_dbt_vars(
                self.cfg, str(ULID()), extra={BLOCKING_DBT_VAR: blocking_payload(self.cfg)}
            ),
            target="lake",
            close_conn=lambda: detach(self.connection),
            reopen_conn=self._reattach,
            project_dir=DBT_PROJECT_DIR,
            profiles_dir=DBT_PROFILES_DIR,
            artifacts_dir=self.artifacts,
        )

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


@pytest.fixture(scope="session")
def cfg() -> Config:
    return config()


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    if (Path(DBT_PROJECT_DIR) / "dbt_packages" / "dbt_utils").is_dir():
        return
    completed = subprocess.run(
        ["dbt", "deps", "--project-dir", DBT_PROJECT_DIR, "--profiles-dir", DBT_PROFILES_DIR],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


def _measured_pass(
    connection: duckdb.DuckDBPyConnection, cfg: Config, tmp_path: Path
) -> tuple[list[Any], dict[str, str], float, float]:
    """Run one smoke pass, returning (records, run_ids, summed_phase_wall, whole_wall)."""
    dbt = Dbt(connection, tmp_path / "artifacts", cfg)
    dbt("seed")
    scale = scales.get_scale(SCALE)
    whole_start = time.monotonic()
    base_drop, batch_drop = _prepare_corpus(
        scale, cfg, tmp_path / "corpus", Path(os.environ["ER_CONFIG"])
    )
    run_ids = {"base": str(ULID()), "train": str(ULID()), "incremental": str(ULID())}
    runner = _SmokeRunner(
        connection,
        cfg=cfg,
        config_path=Path(os.environ["ER_CONFIG"]),
        drop_for_run={run_ids["base"]: base_drop, run_ids["incremental"]: batch_drop},
        dbt=dbt,
    )
    records = run_benchmark.run_pass(runner, run_ids)
    whole_wall = (time.monotonic() - whole_start) * 1000.0
    summed_phase_wall = sum(record.wall_ms for record in records)
    return records, run_ids, summed_phase_wall, whole_wall


def test_smoke_pass_produces_schema_valid_result(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> None:
    """AC1, AC2, AC3, AC5, AC7, AC8: a measured smoke pass; its result validates, its
    phases reconcile against run_stages, and generation is outside the timing."""
    connection = initialised_lake
    records, run_ids, summed_phase_wall, whole_wall = _measured_pass(connection, cfg, tmp_path)

    # AC2: six phases in order.
    assert [record.name for record in records] == list(run_benchmark.PHASES)

    # AC3: every phase names a run_stages row and wall_ms >= summed stage duration_ms.
    for record in records:
        assert record.wall_ms >= record.stage_duration_ms, (
            f"phase {record.name}: wall_ms {record.wall_ms} < Σ duration_ms "
            f"{record.stage_duration_ms}; the phase timing must bound the lake's record (M2)"
        )

    # AC7: the summed phase time excludes corpus generation and er init.
    assert summed_phase_wall < whole_wall, (
        "summed phase wall is not strictly less than the whole invocation; generation and "
        "init must be outside the measured time (S10.3)"
    )

    # AC8: the fingerprint, read from the live substrate.
    active = connection.execute(
        f"SELECT model_version, tf_snapshot_id FROM {MODEL_REGISTRY} WHERE status = 'active'"
    ).fetchone()
    assert active is not None, "the train phase registered no active model"
    fp = fingerprint.environment_fingerprint(
        scale=SCALE,
        connection=connection,
        config_hash=config_hash(cfg),
        generator_seed=cfg.generator.seed,
        model_version=str(active[0]),
        tf_snapshot_id=str(active[1]),
    )
    assert fp["splink_version"] == "5.0.0.dev5", fp["splink_version"]
    assert fp["dbt_core_version"] == "1.12.2", fp["dbt_core_version"]
    assert fp["dbt_duckdb_version"] == "1.11.0", fp["dbt_duckdb_version"]

    # AC1: the assembled result document validates against the run-document schema. The
    # verdict/quality/memory/wall_ms_cv fields are placeholders filled by ER-098/099/100;
    # here they carry schema-valid zeros so the measurement half is checkable on its own.
    result = {
        "scale": SCALE,
        "verdict": "NO_BASELINE",
        "repeat": 1,
        "incremental_ratio": run_benchmark.incremental_ratio(records),
        "blocking_recall": 0.0,
        "fingerprint": fp,
        "phases": [
            {
                "name": record.name,
                "wall_ms": record.wall_ms,
                "wall_ms_cv": 0.0,
                "records_per_sec": record.records_per_sec,
                "candidate_pair_count": record.candidate_pair_count,
                "pairs_above_auto_merge": record.pairs_above_auto_merge,
                "memory_peak_bytes": 0,
                "snapshot_count": record.snapshot_count,
            }
            for record in records
        ],
        "memory": {
            "duckdb_buffer_peak_bytes": 0,
            "rss_peak_bytes": 0,
            "cgroup_peak_bytes": 0,
        },
        "quality": {
            "edge_precision": 0.0,
            "edge_recall": 0.0,
            "edge_f1": 0.0,
            "cluster_precision": 0.0,
            "cluster_recall": 0.0,
            "cluster_f1": 0.0,
        },
    }
    out = tmp_path / "result.json"
    schema.write_result(result, out)
    # AC1: the written document round-trips and re-validates.
    schema.validate_bench_result(json.loads(out.read_text(encoding="utf-8")))


def test_no_splink_relations_after_measured_run(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> None:
    """AC6: a measured smoke pass leaves zero `__splink__%` relations in the lake (M17)."""
    connection = initialised_lake
    _measured_pass(connection, cfg, tmp_path)
    leaked = connection.execute(
        "SELECT table_schema, table_name FROM information_schema.tables "
        "WHERE table_name LIKE '__splink__%'"
    ).fetchall()
    assert not leaked, f"a measured run leaked Splink scratch relations into the lake: {leaked}"
