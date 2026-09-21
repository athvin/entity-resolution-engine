"""Integration arm of the S11 coherence seam (ER-104): the assemble hook scores the run's
rebuilt entities exactly once and routes findings to entity-scoped `review_queue` rows,
while the `noop` default writes nothing and adds no counter (M20, M2).

The pipeline through reconcile runs as subprocesses, but `assemble()` is called IN-PROCESS
so the `FakeScorer` the test registers is the one the hook constructs — a subprocess `er`
would only ever see the `noop` registry. Follows the ER-092 `SPEC_TEST_IDS` convention.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.config_mutation import mutated_config
from helpers.model import load_fixture_model
from helpers.scenario import Scenario, load_scenario
from helpers.scorers import register_fake
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, render_dbt_vars, run_dbt
from er.entities.ids import MonotonicUlidFactory
from er.errors import ExitCode
from er.golden.assemble import assemble, compute_touched_set
from er.lake.ducklake import attach_statements, connect, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.matching.full import MODE_FULL
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.obs.runctx import DECLARED_COUNTERS, StageCounters
from er.review.queue import COHERENCE, upsert_entity_finding

SPEC_TEST_IDS: Final[tuple[str, ...]] = (
    "tests/integration/test_coherence.py::test_finding_lands_as_entity_review_row",
)

REPO_ROOT: Final = Path(__file__).resolve().parents[2]
SCENARIO: Final = "merge_scenario"
REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.review_queue"
TOUCHED: Final = f"{SCHEMA_QUALIFIER}.er_touched_entities"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"
#: S4.6's closed counter set for the assemble stage (AC6).
ASSEMBLE_COUNTERS: Final = frozenset(DECLARED_COUNTERS["assemble"])


def config() -> Config:
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    payload, _ = blocking_rules_from_config(cfg)
    return payload


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def deliver(scenario: Scenario, phase: str, root: Path) -> Path:
    for source, path in scenario.inputs_for(phase).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


class Dbt:
    def __init__(self, connection: duckdb.DuckDBPyConnection, artifacts: Path, cfg: Config) -> None:
        self.connection = connection
        self.artifacts = artifacts
        self.cfg = cfg

    def build(self, select: str) -> None:
        run_dbt(
            "build",
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

    def seed(self) -> None:
        run_dbt(
            "seed",
            select=None,
            vars=render_dbt_vars(self.cfg, str(ULID())),
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


class _Merged:
    """merge_scenario driven base then batch through reconcile (NOT assemble); the batch
    run's touched set holds the entity the bridge merged."""

    def __init__(self, connection: duckdb.DuckDBPyConnection, cfg: Config, dbt: Dbt, root: Path):
        self.connection = connection
        self.cfg = cfg
        self.dbt = dbt
        self.root = root
        self.scenario = load_scenario(SCENARIO)
        self.base_run_id = ""
        self.batch_run_id = ""

    def _phase_through_reconcile(self, phase: str) -> str:
        run_id = str(ULID())
        delivery = deliver(self.scenario, phase, self.root / phase)
        for source in self.scenario.inputs_for(phase):
            result = run_er(
                "ingest", "--source", source, "--path", str(delivery), "--run-id", run_id
            )
            assert result.returncode in (int(ExitCode.SUCCESS), int(ExitCode.NOTHING_TO_DO)), (
                result.stdout + result.stderr
            )
        self.dbt.build("staging")
        self.dbt.build("intermediate")
        assert run_er("match", "--mode", MODE_FULL, "--run-id", run_id).returncode == 0
        assert run_er("reconcile", "--run-id", run_id).returncode == 0
        return run_id

    def build(self) -> None:
        self.dbt.seed()
        self.base_run_id = self._phase_through_reconcile("base")
        # Base assemble (noop, subprocess) materialises the two base entities' golden rows.
        assert run_er("assemble", "--run-id", self.base_run_id).returncode in (0, 10)
        self.batch_run_id = self._phase_through_reconcile("batch")

    def rebuild_ids(self, run_id: str) -> list[str]:
        return sorted(
            e for e, d in compute_touched_set(self.connection, run_id).items() if d == "rebuild"
        )


@pytest.fixture
def merged(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: Any,
    tmp_path: Path,
) -> Iterator[_Merged]:
    connection = initialised_lake
    dbt = Dbt(connection, tmp_path / "artifacts", cfg)
    model_version, _, settings = load_fixture_model(connection)
    published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
    object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
    connection.execute(
        f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
        [published, model_version],
    )
    pipeline = _Merged(connection, cfg, dbt, tmp_path / "drop")
    pipeline.build()
    yield pipeline


def _assemble_in_process(connection: duckdb.DuckDBPyConnection, cfg: Config, run_id: str) -> Any:
    counters = StageCounters(DECLARED_COUNTERS["assemble"])
    detach(connection)
    try:
        return assemble(
            connect,
            cfg,
            run_id=run_id,
            counters=counters,
            touched_only=True,
            id_factory=MonotonicUlidFactory(),
        ), counters
    finally:
        for statement in attach_statements():
            connection.execute(statement)


def test_scorer_called_once_with_rebuild_touched_ids(merged: _Merged, tmp_path: Path) -> None:
    """AC4: the scorer is constructed and called exactly once, with the run's rebuilt ids
    in ascending order — and no retire entity."""
    rebuild = merged.rebuild_ids(merged.batch_run_id)
    assert rebuild, "the batch phase merged nothing, so there is no rebuilt entity to score"
    state = register_fake(threshold=0.5, dispersion_by_entity=dict.fromkeys(rebuild, 0.9))
    fake_cfg = load_config(
        mutated_config(
            Path(os.environ["ER_CONFIG"]), {"coherence.scorer": "fake"}, dest_dir=tmp_path
        )
    )
    _assemble_in_process(merged.connection, fake_cfg, merged.batch_run_id)

    assert state.constructions == 1, f"scorer constructed {state.constructions} times"
    assert len(state.calls) == 1, f"score_clusters called {len(state.calls)} times"
    queried = [
        str(r[0])
        for r in merged.connection.execute(
            f"SELECT entity_id FROM {TOUCHED} WHERE run_id = ? AND disposition = 'rebuild' "
            "ORDER BY entity_id",
            [merged.batch_run_id],
        ).fetchall()
    ]
    assert state.calls[0] == queried


def test_finding_lands_as_entity_review_row(merged: _Merged, tmp_path: Path) -> None:
    """AC3: a finding above threshold is one entity-scoped review_queue row."""
    rebuild = merged.rebuild_ids(merged.batch_run_id)
    register_fake(threshold=0.5, dispersion_by_entity=dict.fromkeys(rebuild, 0.9))
    fake_cfg = load_config(
        mutated_config(
            Path(os.environ["ER_CONFIG"]), {"coherence.scorer": "fake"}, dest_dir=tmp_path
        )
    )
    _assemble_in_process(merged.connection, fake_cfg, merged.batch_run_id)

    rows = merged.connection.execute(
        "SELECT subject_type, reason, status, rec_a_key, rec_b_key, entity_id, waterfall, "
        f"first_seen_run_id, last_seen_run_id FROM {REVIEW_QUEUE} WHERE reason = ?",
        [COHERENCE],
    ).fetchall()
    assert len(rows) == 1, f"expected one coherence row, got {len(rows)}"
    subject_type, reason, status, rec_a, rec_b, entity_id, waterfall, first_seen, last_seen = rows[
        0
    ]
    assert subject_type == "entity" and reason == COHERENCE and status == "open"
    assert rec_a is None and rec_b is None
    assert entity_id in rebuild
    assert first_seen == merged.batch_run_id == last_seen
    payload = json.loads(waterfall) if isinstance(waterfall, str) else waterfall
    assert payload["dispersion"] == 0.9 and "outlier_record_keys" in payload


def test_rerun_refreshes_and_skips_resolved_subject(merged: _Merged, tmp_path: Path) -> None:
    """AC5: a second scoring refreshes last_seen_run_id without inserting a second row; a
    resolved subject is skipped on the next run."""
    rebuild = merged.rebuild_ids(merged.batch_run_id)
    register_fake(threshold=0.5, dispersion_by_entity=dict.fromkeys(rebuild, 0.9))
    fake_cfg = load_config(
        mutated_config(
            Path(os.environ["ER_CONFIG"]), {"coherence.scorer": "fake"}, dest_dir=tmp_path
        )
    )
    _assemble_in_process(merged.connection, fake_cfg, merged.batch_run_id)
    entity_id = rebuild[0]

    # A second finding under a new run refreshes last_seen_run_id and inserts no row.
    second_run = str(ULID())
    upsert_entity_finding(
        merged.connection, entity_id=entity_id, run_id=second_run, waterfall={"dispersion": 0.9}
    )
    rows = merged.connection.execute(
        f"SELECT count(*), max(last_seen_run_id) FROM {REVIEW_QUEUE} "
        "WHERE reason = ? AND entity_id = ?",
        [COHERENCE, entity_id],
    ).fetchone()
    assert rows is not None and rows[0] == 1 and rows[1] == second_run

    # Resolve it, then a third finding must skip it (no resurrection).
    merged.connection.execute(
        f"UPDATE {REVIEW_QUEUE} SET status = 'dismissed' WHERE reason = ? AND entity_id = ?",
        [COHERENCE, entity_id],
    )
    third_run = str(ULID())
    result = upsert_entity_finding(
        merged.connection, entity_id=entity_id, run_id=third_run, waterfall={"dispersion": 0.9}
    )
    assert result.skipped and not result.inserted and not result.refreshed
    status = merged.connection.execute(
        f"SELECT status FROM {REVIEW_QUEUE} WHERE reason = ? AND entity_id = ?",
        [COHERENCE, entity_id],
    ).fetchone()
    assert status is not None and status[0] == "dismissed"


def test_noop_writes_no_rows_and_no_extra_counters(merged: _Merged) -> None:
    """AC6: under the noop default the hook writes no coherence row, and the assemble
    stage adds only its S4.6 counters and the shared profiling counts/units."""
    _, counters = _assemble_in_process(merged.connection, merged.cfg, merged.batch_run_id)
    count = merged.connection.execute(
        f"SELECT count(*) FROM {REVIEW_QUEUE} WHERE reason = ?", [COHERENCE]
    ).fetchone()
    assert count is not None and count[0] == 0
    payload = counters.payload()
    assert set(payload) == ASSEMBLE_COUNTERS | {"rows_in", "rows_out", "input_unit", "output_unit"}
    assert payload["rows_in"] == payload["entities_touched"]
    assert payload["rows_out"] == payload["entities_rebuilt"]
    assert payload["input_unit"] == payload["output_unit"] == "entities"
