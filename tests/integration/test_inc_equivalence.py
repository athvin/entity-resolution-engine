"""T-INC-1: incremental equals full under the INV-EQ preconditions (G3, S4.5.6, S8.3).

Two isolated universes from `sub_namespace`: **A** runs `base ∪ batch` in one full
pass; **B** runs `base` then `batch` incrementally. S4.5.6's INV-EQ says that under a
pinned model, a pinned TF snapshot, an unchanged `std_version`/`config_hash`, an
identical active assertion set and an append-only corpus, the two produce the same
set-partition of current members. The preconditions are asserted BEFORE the comparison
— each with its own message — so a divergence caused by one of them reads as a violated
premise rather than as a broken invariant.

Entity ids legitimately differ between the two universes and are NEVER compared: each
arm is compared against the ONE committed ID-insensitive expectation
(`expected/batch/membership.csv` and `expected/batch/golden.csv`) through its own
label map, and equality of both to one expectation is equality to each other (M7).

`assert_partition_equal` compares frozensets of `record_key`, so a run that re-minted
every entity would still pass it — which is exactly why the sensitivity arm below,
which perturbs a partition and demands the comparison raise, is mandatory.

Follows the ER-092 `SPEC_TEST_IDS` convention.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import Any, Final

import duckdb
import pandas as pd
import pytest
from helpers.compare import assert_golden_equal, assert_partition_equal
from helpers.expected import label_map_from_membership
from helpers.model import load_fixture_model
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

#: The S8.3 rows this module realises (ER-092 convention).
SPEC_TEST_IDS: Final[tuple[str, ...]] = (
    "tests/integration/test_incremental.py::test_incremental_equals_full",
)

REPO_ROOT: Final = Path(__file__).resolve().parents[2]
SCENARIO_NAME: Final = "incremental_batch"
BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
GOLDEN_RECORDS: Final = f"{SCHEMA_QUALIFIER}.golden_records"
RUNS: Final = f"{SCHEMA_QUALIFIER}.runs"
ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.assertions"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

FIXTURE_ROOT: Final = REPO_ROOT / "fixtures" / "static"
EXPECTED_MEMBERSHIP: Final = (
    FIXTURE_ROOT / SCENARIO_NAME / "expected" / BATCH_PHASE / "membership.csv"
)
EXPECTED_GOLDEN: Final = FIXTURE_ROOT / SCENARIO_NAME / "expected" / BATCH_PHASE / "golden.csv"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

#: The golden_records value columns the committed expectation compares (S8.2.1).
GOLDEN_VALUE_COLUMNS: Final = (
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
    "survivorship_version",
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


def deliver(scenario: Scenario, phase: str, root: Path) -> Path:
    for source, path in scenario.inputs_for(phase).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


class Dbt:
    def __init__(self, connection: duckdb.DuckDBPyConnection, artifacts: Path, cfg: Config):
        self.connection = connection
        self.artifacts = artifacts
        self.cfg = cfg

    def __call__(self, command: str, select: str | None = None) -> DbtResult:
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

    def standardize(self, *, changed_only: bool) -> None:
        self("build", select=STAGING_SELECTOR)
        self("build", select=INTERMEDIATE_SELECTOR)

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


def _publish_model(
    connection: duckdb.DuckDBPyConnection, cfg: Config, object_store: ObjectStore
) -> tuple[str, str]:
    model_version, tf_snapshot_id, settings = load_fixture_model(connection)
    published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
    object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
    connection.execute(
        f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
        [published, model_version],
    )
    return model_version, tf_snapshot_id


class Universe:
    """One lake namespace, driven a phase at a time, holding its own state."""

    def __init__(
        self,
        connection: duckdb.DuckDBPyConnection,
        cfg: Config,
        scenario: Scenario,
        root: Path,
        artifacts: Path,
    ) -> None:
        self.connection = connection
        self.cfg = cfg
        self.scenario = scenario
        self.root = root
        self.dbt = Dbt(connection, artifacts, cfg)
        self.run_ids: list[str] = []

    def phase(self, phases: tuple[str, ...], *, mode: str) -> None:
        run_id = str(ULID())
        self.run_ids.append(run_id)
        for phase in phases:
            delivery = deliver(self.scenario, phase, self.root / phase)
            for source in self.scenario.inputs_for(phase):
                result = run_er(
                    "ingest", "--source", source, "--path", str(delivery), "--run-id", run_id
                )
                assert result.returncode in (
                    int(ExitCode.SUCCESS),
                    int(ExitCode.NOTHING_TO_DO),
                ), result.stdout + result.stderr
        self.dbt.standardize(changed_only=mode == "incremental")
        for stage_args in (
            ("match", "--mode", mode, "--run-id", run_id, "--json"),
            ("reconcile", "--run-id", run_id),
            (
                "assemble",
                *(("--touched-only",) if mode == "incremental" else ()),
                "--run-id",
                run_id,
            ),
        ):
            result = run_er(*stage_args)
            assert result.returncode in (
                int(ExitCode.SUCCESS),
                int(ExitCode.NOTHING_TO_DO),
            ), result.stdout + result.stderr

    def membership(self) -> list[tuple[str, str]]:
        return [
            (str(record), str(entity))
            for record, entity in self.connection.execute(
                f"SELECT record_key, entity_id FROM {MEMBERSHIP} ORDER BY record_key"
            ).fetchall()
        ]

    def golden(self) -> pd.DataFrame:
        columns = ("entity_id", *GOLDEN_VALUE_COLUMNS)
        rows = self.connection.execute(
            f"SELECT {', '.join(columns)} FROM {GOLDEN_RECORDS}"
        ).fetchall()
        return pd.DataFrame(rows, columns=list(columns))

    def fingerprint(self) -> dict[str, Any]:
        """The INV-EQ preconditions this universe ran under (S4.5.6).

        `model_version`/`tf_snapshot_id` come from the active model row, `config_hash`
        from this arm's last `runs` row, `std_version` from the config both arms share,
        and the active assertion set from `assertions`. Appended-only corpus and
        no-deletion are structural properties of the fixture (its batch adds rows and
        tombstones none), so they are documented rather than queried.
        """
        active = self.connection.execute(
            f"SELECT model_version, tf_snapshot_id FROM {MODEL_REGISTRY} WHERE status = 'active'"
        ).fetchall()
        assert len(active) == 1
        config_hash = str(
            scalar(
                self.connection,
                f"SELECT config_hash FROM {RUNS} WHERE run_id = ?",
                self.run_ids[-1],
            )
        )
        assertions = sorted(
            (str(a), str(b))
            for a, b in self.connection.execute(
                f"SELECT rec_a_key, rec_b_key FROM {ASSERTIONS} WHERE active"
            ).fetchall()
        )
        return {
            "model_version": str(active[0][0]),
            "tf_snapshot_id": str(active[0][1]),
            "config_hash": config_hash,
            "std_version": str(self.cfg.versions.std_version),
            "active_assertions": assertions,
        }


def assert_inv_eq_preconditions(a: dict[str, Any], b: dict[str, Any]) -> None:
    """The four INV-EQ preconditions of S4.5.6, each failure naming its field."""
    for field in ("model_version", "tf_snapshot_id", "config_hash", "std_version"):
        assert a[field] == b[field], (
            f"INV-EQ precondition '{field}' differs between the arms: full={a[field]!r}, "
            f"incremental={b[field]!r}; a difference here is a violated premise, not a "
            "broken invariant (S4.5.6)"
        )
    assert a["active_assertions"] == b["active_assertions"], (
        f"the active assertion set differs: full={a['active_assertions']}, "
        f"incremental={b['active_assertions']}"
    )


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


def _drive(
    universe_obj: Any,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
    suffix: str,
    *,
    mode: str,
) -> Universe:
    """Run the scenario in ``universe_obj`` with the process env pointed at it."""
    scenario = load_scenario(SCENARIO_NAME)
    with pytest.MonkeyPatch.context() as patch:
        for name, value in universe_obj.env.items():
            patch.setenv(name, value)
        dbt = Dbt(universe_obj.connection, tmp_path / f"artifacts_{suffix}", cfg)
        dbt("seed")
        _publish_model(universe_obj.connection, cfg, object_store)
        run = Universe(
            universe_obj.connection,
            cfg,
            scenario,
            tmp_path / f"drop_{suffix}",
            tmp_path / f"artifacts_{suffix}",
        )
        if mode == "full":
            run.phase((BASE_PHASE, BATCH_PHASE), mode="full")
        else:
            run.phase((BASE_PHASE,), mode="incremental")
            run.phase((BATCH_PHASE,), mode="incremental")
    return run


def test_incremental_equals_full(
    dbt_packages: None,
    sub_namespace: Any,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> None:
    """T-INC-1 / AC2, AC3, AC4: both arms equal the one committed expectation."""
    universe_a = sub_namespace("a")
    universe_b = sub_namespace("b")

    # AC3: the two namespaces are genuinely distinct — a different METADATA_SCHEMA
    # and DATA_PATH each (S7.2), which is the pair `sub_namespace` derived per suffix.
    assert universe_a.namespace.metadata_schema != universe_b.namespace.metadata_schema, (
        "the two universes share a metadata schema"
    )
    assert universe_a.namespace.data_path != universe_b.namespace.data_path, (
        "the two universes share a data path"
    )

    full = _drive(universe_a, cfg, object_store, tmp_path, "a", mode="full")
    incremental = _drive(universe_b, cfg, object_store, tmp_path, "b", mode="incremental")

    # AC2: the INV-EQ preconditions, asserted BEFORE any partition is compared.
    assert_inv_eq_preconditions(full.fingerprint(), incremental.fingerprint())

    # AC4: each arm against the ONE committed expectation, through its OWN label map.
    for arm in (full, incremental):
        assert_partition_equal(arm.membership(), EXPECTED_MEMBERSHIP)
        label_map = label_map_from_membership(arm.membership())
        assert_golden_equal(arm.golden(), EXPECTED_GOLDEN, label_map)


def test_inv_eq_preconditions_are_asserted() -> None:
    """AC2's negative arm: a perturbed precondition fails with its field named."""
    good = {
        "model_version": "v0001",
        "tf_snapshot_id": "SNAP1",
        "config_hash": "abc",
        "std_version": "1",
        "active_assertions": [],
    }
    assert_inv_eq_preconditions(good, dict(good))  # identical arms pass

    for field in ("model_version", "tf_snapshot_id", "config_hash", "std_version"):
        perturbed = dict(good)
        perturbed[field] = "moved"
        with pytest.raises(AssertionError, match=field):
            assert_inv_eq_preconditions(good, perturbed)

    with pytest.raises(AssertionError, match="active assertion set"):
        assert_inv_eq_preconditions(good, {**good, "active_assertions": [("s:a", "s:b")]})


def test_partition_comparison_is_sensitive(tmp_path: Path) -> None:
    """AC5: `assert_partition_equal` raises on a merged partition — not vacuous.

    A pure check of the comparison helper against the committed expectation: merging
    two of its entities must make the ID-insensitive comparison fail, or a pipeline
    that co-clustered everything would pass T-INC-1 silently.
    """
    import csv as _csv

    with EXPECTED_MEMBERSHIP.open(newline="") as handle:
        rows = list(_csv.DictReader(handle))
    labels = sorted({row["entity_label"] for row in rows})
    assert len(labels) >= 2, "the expectation has too few entities to merge"

    # The committed partition, unperturbed, passes.
    faithful = [
        (f"{row['source_system']}:{row['source_record_id']}", row["entity_label"]) for row in rows
    ]
    assert_partition_equal(faithful, EXPECTED_MEMBERSHIP)

    # Collapsing two entities into one must make the ID-insensitive comparison raise.
    absorbed = labels[1]
    merged = [(key, labels[0] if label == absorbed else label) for key, label in faithful]
    with pytest.raises(AssertionError):
        assert_partition_equal(merged, EXPECTED_MEMBERSHIP)
