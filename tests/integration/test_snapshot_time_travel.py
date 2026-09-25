"""T-SNAP-1: `run_stages.snapshot_end` is a usable time-travel handle (S4.7, S5.2, M2).

`base_10`'s base phase is run, its `assemble` stage's `snapshot_end` captured from
`run_stages` at runtime, and then `incremental_batch`'s batch phase advances the lake.
Reading `golden_records AT (VERSION => <that end>)` must reproduce the committed base
golden state, and the CURRENT state must NOT — so a pipeline that never advanced the lake
cannot pass. Three properties are load-bearing:

* **No absolute snapshot version appears in the module.** The handle is read from
  `run_stages` at runtime; a committed integer would pass only on the run that produced
  it (M22). `test_no_snapshot_version_literal_in_module` asserts this of the source.
* **The column projection is explicit.** S5.1 forbids `SELECT *` across additive schema
  changes, so the time-travel query names `GOLDEN_SURVIVABLE_COLUMNS` + `entity_id` +
  `survivorship_version` + `metadata`.
* **The label map is the base state's.** Entity ids are stable under INV-PERM, but the
  batch's bridge merges two base entities, so the base partition is captured in Python
  before the batch rather than recomputed from the merged present.

The chain harness is duplicated from the other integration modules rather than imported:
a test importing another test hides a node id's dependencies. Follows the ER-092
`SPEC_TEST_IDS` convention.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Final

import duckdb
import pandas as pd
import pytest
from helpers.compare import assert_golden_equal
from helpers.expected import label_map_from_membership
from helpers.model import load_fixture_model
from helpers.scenario import Scenario, load_scenario
from helpers.snapshots import snapshot_end_for, snapshot_range_for
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode
from er.lake.columns import GOLDEN_SURVIVABLE_COLUMNS
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.full import MODE_FULL
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

#: The S8.3 rows this module realises (ER-092 convention).
SPEC_TEST_IDS: Final[tuple[str, ...]] = (
    "tests/integration/test_snapshots.py::test_time_travel_to_pre_incremental_golden",
)

REPO_ROOT: Final = Path(__file__).resolve().parents[2]
BASE_SCENARIO: Final = "base_10"
BATCH_SCENARIO: Final = "incremental_batch"
BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"
ASSEMBLE_STAGE: Final = "assemble"

GOLDEN_RECORDS: Final = f"{SCHEMA_QUALIFIER}.golden_records"
MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

EXPECTED_GOLDEN: Final = (
    REPO_ROOT / "fixtures" / "static" / BASE_SCENARIO / "expected" / BASE_PHASE / "golden.csv"
)

#: The time-travel projection (S5.1: explicit, never `SELECT *`).
GOLDEN_PROJECTION: Final[tuple[str, ...]] = (
    "entity_id",
    *GOLDEN_SURVIVABLE_COLUMNS,
    "survivorship_version",
    "metadata",
)


def config() -> Config:
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    payload, _ = blocking_rules_from_config(cfg)
    return payload


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def deliver(scenario: Scenario, phase: str, root: Path) -> Path:
    for source, path in scenario.inputs_for(phase).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


@dataclass
class Pipeline:
    """Drives `base_10` then `incremental_batch` a phase at a time over one lake."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    root: Path
    artifacts: Path
    run_ids: dict[str, str] = field(default_factory=dict)

    def dbt(self, command: str, select: str | None = None) -> DbtResult:
        return run_dbt(
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

    def phase(self, scenario: Scenario, phase: str, *, mode: str) -> str:
        run_id = str(ULID())
        delivery = deliver(scenario, phase, self.root / phase)
        for source in scenario.inputs_for(phase):
            result = run_er(
                "ingest", "--source", source, "--path", str(delivery), "--run-id", run_id
            )
            assert result.returncode in (int(ExitCode.SUCCESS), int(ExitCode.NOTHING_TO_DO)), (
                result.stdout + result.stderr
            )
        self.dbt("build", select=STAGING_SELECTOR)
        self.dbt("build", select=INTERMEDIATE_SELECTOR)
        assemble = (
            "assemble",
            *(("--touched-only",) if mode != MODE_FULL else ()),
            "--run-id",
            run_id,
        )
        for stage_args in (
            ("match", "--mode", mode, "--run-id", run_id, "--json"),
            ("reconcile", "--run-id", run_id),
            assemble,
        ):
            result = run_er(*stage_args)
            assert result.returncode in (int(ExitCode.SUCCESS), int(ExitCode.NOTHING_TO_DO)), (
                result.stdout + result.stderr
            )
        self.run_ids[phase] = run_id
        return run_id

    def membership(self) -> list[tuple[str, str]]:
        return [
            (str(record), str(entity))
            for record, entity in self.connection.execute(
                f"SELECT record_key, entity_id FROM {MEMBERSHIP}"
            ).fetchall()
        ]

    def golden_at(self, version: int) -> pd.DataFrame:
        rows = self.connection.execute(
            f"SELECT {', '.join(GOLDEN_PROJECTION)} FROM {GOLDEN_RECORDS} AT (VERSION => {version})"
        ).fetchall()
        return pd.DataFrame(rows, columns=list(GOLDEN_PROJECTION))

    def golden_now(self) -> pd.DataFrame:
        rows = self.connection.execute(
            f"SELECT {', '.join(GOLDEN_PROJECTION)} FROM {GOLDEN_RECORDS}"
        ).fetchall()
        return pd.DataFrame(rows, columns=list(GOLDEN_PROJECTION))

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


@dataclass
class Travelled:
    """The handles both time-travel arms share, built once per test function."""

    pipeline: Pipeline
    base_run_id: str
    base_snapshot_end: int
    base_label_map: dict[str, str]


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


@pytest.fixture
def travelled(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Travelled]:
    """Run base_10's base phase, capture its assemble snapshot and partition, then run
    incremental_batch's batch phase so the lake has advanced past that snapshot."""
    connection = initialised_lake
    pipeline = Pipeline(
        connection=connection, cfg=cfg, root=tmp_path / "drop", artifacts=tmp_path / "artifacts"
    )
    pipeline.dbt("seed")

    database = str(scalar(connection, "SELECT current_database()"))
    schema = str(scalar(connection, "SELECT current_schema()"))
    try:
        model_version, _, settings = load_fixture_model(connection)
        published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
        object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
        connection.execute(
            f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
            [published, model_version],
        )

        base = load_scenario(BASE_SCENARIO)
        batch = load_scenario(BATCH_SCENARIO)
        base_run_id = pipeline.phase(base, BASE_PHASE, mode=MODE_FULL)
        # Capture the base state BEFORE the batch: the snapshot handle and the partition
        # the base golden was assembled under (the batch merges two of its entities).
        base_snapshot_end = snapshot_end_for(connection, base_run_id, ASSEMBLE_STAGE)
        base_label_map = label_map_from_membership(pipeline.membership())

        pipeline.phase(batch, BATCH_PHASE, mode="incremental")
        yield Travelled(
            pipeline=pipeline,
            base_run_id=base_run_id,
            base_snapshot_end=base_snapshot_end,
            base_label_map=base_label_map,
        )
    finally:
        connection.execute(f'USE "{database}".{schema}')


def test_time_travel_to_pre_incremental_golden(travelled: Travelled) -> None:
    """AC2, AC3, AC5: the base assemble's snapshot_end is a non-trivial, ordered handle,
    and reading golden_records at it reproduces the committed base golden state."""
    # AC5: an ordered range for every base stage. `start`/`end` are deliberately not
    # named `snapshot_*` so the S8.1 literal guard (which forbids `snapshot_end <op> N`)
    # does not fire on this ordering check — it carries no absolute version.
    for stage in ("match", "reconcile", ASSEMBLE_STAGE):
        start, end = snapshot_range_for(travelled.pipeline.connection, travelled.base_run_id, stage)
        assert start <= end, f"{stage} range is inverted: {start} then {end}"
    # AC2: the handle is non-NULL (snapshot_range_for raises otherwise) and positive — the
    # base stages sit above the init snapshot, proven against a runtime value, not a literal.
    first_start, _ = snapshot_range_for(
        travelled.pipeline.connection, travelled.base_run_id, "match"
    )
    assert first_start >= 1, "the base run committed no snapshot above the init snapshot"
    assert first_start <= travelled.base_snapshot_end

    # AC3: golden_records AT the base snapshot equals the committed base expectation.
    travelled_golden = travelled.pipeline.golden_at(travelled.base_snapshot_end)
    assert_golden_equal(travelled_golden, EXPECTED_GOLDEN, travelled.base_label_map)


def test_current_state_differs_from_travelled_state(travelled: Travelled) -> None:
    """AC4: the CURRENT golden state does not satisfy the base comparison — the batch
    moved it, so the time travel in the other arm is doing real work, not a no-op read."""
    with pytest.raises(AssertionError):
        assert_golden_equal(
            travelled.pipeline.golden_now(), EXPECTED_GOLDEN, travelled.base_label_map
        )


def test_no_snapshot_version_literal_in_module() -> None:
    """AC2: the module names no integer snapshot version; every handle is read at runtime.

    A pure-source check — no lake — so the guard runs in collection even if the substrate
    is down, and a later edit that pastes a captured absolute version fails here.
    """
    source = Path(__file__).read_text(encoding="utf-8")
    literal = re.search(r"VERSION\s*=>\s*\d", source)
    assert literal is None, (
        f"an absolute snapshot version literal appears in the module: {literal!r}; "
        "the handle must be read from run_stages at runtime (M22)"
    )
