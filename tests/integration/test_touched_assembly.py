"""T-INC-2: touched-only assembly, the reap step, and `rewritten ∪ reaped == touched`.

S4.6's touched-only assembly rebuilds only the entities a run changed and REAPS the
ones it emptied. M10 named the two ways the naive approach breaks: a `--vars` payload
of entity ids hard-fails with `E2BIG`, and dbt's `delete+insert` cannot delete a key
absent from the incoming batch. This module drives the real `er assemble` through a
merge and a deletion and asserts the accounting T-INC-2 rests on — the set of
entities whose `assembled_at` became the run's stamp, unioned with the set the reap
deleted, equals exactly `er_touched_entities` — plus the reap's effect on all three
marts, the exit-`10` empty path, and the no-entity-id argv.

Test-layout convention (reused by ER-093/094/095): the board's file name wins over
S8.3's `file path` column, the FUNCTION name matches S8.3's node-id, and
:data:`SPEC_TEST_IDS` keeps the S8.3 row machine-resolvable.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator
from datetime import datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import load_fixture_model
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.ids import resolve
from er.errors import ExitCode
from er.golden.assemble import assemble_dbt_vars
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.full import MODE_FULL
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

#: The S8.3 rows this module realises, kept machine-resolvable per the convention above.
SPEC_TEST_IDS: Final[tuple[str, ...]] = (
    "tests/integration/test_incremental.py::test_rewritten_plus_reaped_equals_touched",
)

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

MERGE_SCENARIO: Final = "merge_scenario"
DELETION_SCENARIO: Final = "deletion_scenario"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
ENTITIES: Final = f"{SCHEMA_QUALIFIER}.entities"
RUNS: Final = f"{SCHEMA_QUALIFIER}.runs"
TOUCHED: Final = f"{SCHEMA_QUALIFIER}.er_touched_entities"
GOLDEN_RECORDS: Final = f"{SCHEMA_QUALIFIER}.golden_records"
GOLDEN_LINEAGE: Final = f"{SCHEMA_QUALIFIER}.golden_lineage"
GOLDEN_DISPLAY: Final = f"{SCHEMA_QUALIFIER}.golden_display"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"

MATCH_STAGE: Final = "match"
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

#: 128 KiB — the per-argv-element ceiling M10 is about (AC5).
ARGV_ELEMENT_LIMIT: Final = 128 * 1024


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

    def standardize(self) -> None:
        self("build", select=STAGING_SELECTOR)
        self("build", select=INTERMEDIATE_SELECTOR)

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


class Pipeline:
    """A scenario driven through ingest → standardize → match → reconcile → assemble."""

    def __init__(
        self,
        connection: duckdb.DuckDBPyConnection,
        cfg: Config,
        scenario: Scenario,
        dbt: Dbt,
        root: Path,
    ) -> None:
        self.connection = connection
        self.cfg = cfg
        self.scenario = scenario
        self.dbt = dbt
        self.root = root
        self.run_id = ""

    def phase(self, name: str, *, touched_only: bool, refresh: bool = False) -> None:
        self.run_id = str(ULID())
        delivery = deliver(self.scenario, name, self.root / name)
        flags = ("--full-refresh-keys",) if refresh else ()
        for source in self.scenario.inputs_for(name):
            result = run_er(
                "ingest",
                "--source",
                source,
                "--path",
                str(delivery),
                *flags,
                "--run-id",
                self.run_id,
            )
            assert result.returncode in (
                int(ExitCode.SUCCESS),
                int(ExitCode.NOTHING_TO_DO),
            ), result.stdout + result.stderr
        self.dbt.standardize()
        scored = run_er("match", "--mode", MODE_FULL, "--run-id", self.run_id, "--json")
        assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr
        reconciled = run_er("reconcile", "--run-id", self.run_id)
        assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr
        assembled = run_er(
            "assemble", *(("--touched-only",) if touched_only else ()), "--run-id", self.run_id
        )
        assert assembled.returncode in (
            int(ExitCode.SUCCESS),
            int(ExitCode.NOTHING_TO_DO),
        ), assembled.stdout + assembled.stderr

    def touched_set(self, run_id: str) -> dict[str, str]:
        return {
            str(entity): str(disposition)
            for entity, disposition in self.connection.execute(
                f"SELECT entity_id, disposition FROM {TOUCHED} WHERE run_id = ?", [run_id]
            ).fetchall()
        }

    def assembled_at(self) -> dict[str, datetime]:
        return {
            str(entity): stamp
            for entity, stamp in self.connection.execute(
                f"SELECT entity_id, assembled_at FROM {GOLDEN_RECORDS}"
            ).fetchall()
        }

    def run_started_at(self, run_id: str) -> datetime:
        stamp = scalar(self.connection, f"SELECT started_at FROM {RUNS} WHERE run_id = ?", run_id)
        return stamp if isinstance(stamp, datetime) else datetime.fromisoformat(str(stamp))


def _publish_model(
    connection: duckdb.DuckDBPyConnection, cfg: Config, object_store: ObjectStore
) -> None:
    model_version, _, settings = load_fixture_model(connection)
    published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
    object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
    connection.execute(
        f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
        [published, model_version],
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


def _pipeline(
    name: str,
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Pipeline:
    scenario = load_scenario(name)
    dbt = Dbt(connection, tmp_path / "artifacts", cfg)
    dbt("seed")
    _publish_model(connection, cfg, object_store)
    return Pipeline(connection, cfg, scenario, dbt, tmp_path / "drop")


@pytest.fixture
def merge_pipeline(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Pipeline]:
    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        yield _pipeline(MERGE_SCENARIO, initialised_lake, cfg, object_store, tmp_path)
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


@pytest.fixture
def deletion_pipeline(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Pipeline]:
    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        yield _pipeline(DELETION_SCENARIO, initialised_lake, cfg, object_store, tmp_path)
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def test_rewritten_plus_reaped_equals_touched(merge_pipeline: Pipeline) -> None:
    """T-INC-2 / AC2, AC3, AC7: the touched-set accounting is an equality."""
    merge_pipeline.phase("base", touched_only=False)
    before = merge_pipeline.assembled_at()

    merge_pipeline.phase("batch", touched_only=True)
    run_id = merge_pipeline.run_id
    touched = set(merge_pipeline.touched_set(run_id))
    run_started = merge_pipeline.run_started_at(run_id)

    after = merge_pipeline.assembled_at()
    rewritten = {entity for entity, stamp in after.items() if stamp == run_started}
    reaped = set(before) - set(after)

    assert rewritten | reaped == touched, (
        "rewritten ∪ reaped != touched (T-INC-2)\n"
        f"  only in rewritten∪reaped: {sorted((rewritten | reaped) - touched)}\n"
        f"  only in touched:          {sorted(touched - (rewritten | reaped))}"
    )

    # AC3: every untouched entity's assembled_at is byte-unchanged across the run.
    for entity, stamp in after.items():
        if entity not in touched:
            assert before.get(entity) == stamp, f"{entity} was re-stamped but not touched"

    # AC7: the reconcile-touched counters add up on the assemble stage's row.
    counters = json.loads(
        str(
            scalar(
                merge_pipeline.connection,
                f"SELECT counters FROM {RUN_STAGES} WHERE run_id = ? AND stage = 'assemble'",
                run_id,
            )
        )
    )
    for name in ("entities_touched", "entities_rebuilt", "entities_reaped", "lineage_rows"):
        assert counters.get(name) is not None, f"{name} is NULL in the assemble counters"
    assert (
        counters["entities_rebuilt"] + counters["entities_reaped"] == counters["entities_touched"]
    ), counters


def test_retire_disposition_reaps_all_three_marts(merge_pipeline: Pipeline) -> None:
    """AC4: the merge loser is retired and has zero golden/lineage/display rows."""
    merge_pipeline.phase("base", touched_only=False)
    loser = str(
        scalar(
            merge_pipeline.connection,
            f"SELECT entity_id FROM {MEMBERSHIP} WHERE record_key = 'billing:B501'",
        )
    )
    merge_pipeline.phase("batch", touched_only=True)

    disposition = merge_pipeline.touched_set(merge_pipeline.run_id).get(loser)
    assert disposition == "retire", f"the merge loser {loser} is {disposition}, not retire"
    for relation in (GOLDEN_RECORDS, GOLDEN_LINEAGE, GOLDEN_DISPLAY):
        count = int(
            scalar(
                merge_pipeline.connection,
                f"SELECT count(*) FROM {relation} WHERE entity_id = ?",
                loser,
            )
        )
        assert count == 0, f"{relation} still holds {count} row(s) for the reaped {loser}"

    status = str(
        scalar(
            merge_pipeline.connection,
            f"SELECT status FROM {ENTITIES} WHERE entity_id = ?",
            loser,
        )
    )
    assert status == "merged"
    survivor = str(
        scalar(
            merge_pipeline.connection,
            f"SELECT entity_id FROM {MEMBERSHIP} WHERE record_key = 'crm:C510'",
        )
    )
    redirects = {
        str(entity): str(target)
        for entity, target in merge_pipeline.connection.execute(
            f"SELECT entity_id, merged_into FROM {ENTITIES} WHERE merged_into IS NOT NULL"
        ).fetchall()
    }
    assert resolve(loser, redirects) == survivor


def test_touched_only_with_empty_set_exits_10(merge_pipeline: Pipeline) -> None:
    """AC6: a touched-only assemble over a run that touched nothing exits 10."""
    merge_pipeline.phase("base", touched_only=False)
    before = merge_pipeline.assembled_at()

    empty_run = str(ULID())
    # A run with no reconcile means no events, so its touched set is empty.
    assembled = run_er("assemble", "--touched-only", "--run-id", empty_run)
    assert assembled.returncode == int(ExitCode.NOTHING_TO_DO), assembled.stdout + assembled.stderr
    assert merge_pipeline.assembled_at() == before, "an exit-10 assemble moved a golden row"
    assert (
        int(
            scalar(
                merge_pipeline.connection,
                f"SELECT count(*) FROM {TOUCHED} WHERE run_id = ?",
                empty_run,
            )
        )
        == 0
    )


def test_dbt_vars_carry_no_entity_id_list(merge_pipeline: Pipeline, cfg: Config) -> None:
    """AC5: the assemble --vars payload names no entity id and every element is small."""
    merge_pipeline.phase("base", touched_only=False)
    merge_pipeline.phase("batch", touched_only=True)
    run_id = merge_pipeline.run_id
    touched = list(merge_pipeline.touched_set(run_id))
    assert touched, "the batch touched nothing; the argv check would be vacuous"

    payload = assemble_dbt_vars(
        cfg, run_id, merge_pipeline.run_started_at(run_id), touched_only=True
    )
    encoded = json.dumps(payload)
    for entity_id in touched:
        assert entity_id not in encoded, (
            f"entity id {entity_id} appears in the dbt vars payload; the touched set "
            "must go through er_touched_entities, not the argv (M10)"
        )
    # Every argv element the stage would build stays under the E2BIG ceiling.
    for element in ("build", "--select", "marts", "--vars", encoded, run_id):
        assert len(element.encode("utf-8")) < ARGV_ELEMENT_LIMIT, (
            f"an argv element is {len(element.encode('utf-8'))} bytes, over the "
            f"{ARGV_ELEMENT_LIMIT}-byte limit"
        )


def test_deletion_empties_entity_and_reaps_its_golden_rows(deletion_pipeline: Pipeline) -> None:
    """AC8: the entity a refresh empties is retired and its golden rows are gone."""
    deletion_pipeline.phase("base", touched_only=False)
    singleton = str(
        scalar(
            deletion_pipeline.connection,
            f"SELECT entity_id FROM {MEMBERSHIP} WHERE record_key = 'crm:C323'",
        )
    )
    assert (
        int(
            scalar(
                deletion_pipeline.connection,
                f"SELECT count(*) FROM {GOLDEN_RECORDS} WHERE entity_id = ?",
                singleton,
            )
        )
        == 1
    ), "the base run did not build a golden row for the singleton"

    snapshot = int(
        scalar(
            deletion_pipeline.connection,
            f"SELECT snapshot_end FROM {RUN_STAGES} WHERE run_id = ? AND stage = 'assemble'",
            deletion_pipeline.run_id,
        )
    )

    deletion_pipeline.phase("refresh", touched_only=True, refresh=True)
    assert deletion_pipeline.touched_set(deletion_pipeline.run_id).get(singleton) == "retire"
    for relation in (GOLDEN_RECORDS, GOLDEN_LINEAGE):
        count = int(
            scalar(
                deletion_pipeline.connection,
                f"SELECT count(*) FROM {relation} WHERE entity_id = ?",
                singleton,
            )
        )
        assert count == 0, f"{relation} still holds the emptied entity's rows"

    # The reaped state is still readable at the base run's snapshot (S5.2).
    at_snapshot = int(
        scalar(
            deletion_pipeline.connection,
            f"SELECT count(*) FROM {GOLDEN_RECORDS} AT (VERSION => {snapshot}) WHERE entity_id = ?",
            singleton,
        )
    )
    assert at_snapshot == 1, "the reaped entity's history is not readable at snapshot_end"
