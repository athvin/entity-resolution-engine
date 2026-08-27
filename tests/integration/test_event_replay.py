"""Replay against real runs: the log reproduces current state (S4.5.3, S4.5.4, D3, M3).

S3 promises "append + replay". Nothing until now made that checkable: `entity_events`
is written by reconcile and `entity_membership` is written beside it, and no assertion
related the two. This file is that relation, in four arms:

* **Replay reproduces membership**, over every scenario the repository commits. The
  scenario set is DISCOVERED rather than listed, so a fixture added later is covered the
  day it lands — and one that is missing today (`split_scenario`, whose ticket ER-077 is
  blocked) does not fail collection for a reason that has nothing to do with replay.
* **`seq` is dense and the idempotency key is unique.** Dense because replay orders by
  `(occurred_at, seq)` and a gap is indistinguishable from a lost event; unique because
  `(run_id, entity_id, event_type, details_hash)` is what makes "a re-run producing
  identical output writes zero events" a checkable claim rather than an intention.
* **An unchanged re-run writes nothing and exits 10.** The exit code matters as much as
  the row count: a run that wrote zero events because it *failed* satisfies a count-only
  assertion, and S4.0 gives `10` the specific meaning "nothing to do".
* **Point-in-time replay matches a time-travelled read.** Folding up to run N and
  comparing against `entity_membership AT (VERSION => …)` at run N's recorded snapshot
  is the strongest form of the claim: the log reconstructs a PAST state, not merely the
  present one.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.invariants import assert_replay_reproduces_membership
from helpers.model import load_fixture_model
from helpers.scenario import Scenario, discover_scenarios, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import (
    DBT_PROFILES_DIR,
    DBT_PROJECT_DIR,
    DbtResult,
    render_dbt_vars,
    run_dbt,
)
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.full import MODE_FULL
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

#: The committed-fixture root. Deliberately NOT `SCENARIO_ROOTS`: that tuple also names
#: `tests/fixtures/scenarios/`, which holds the validator's negative fixtures — a
#: `bad_header` or a `cycle_a` is authored to be rejected, and driving one through the
#: pipeline would fail for the reason it exists rather than for anything about replay.
STATIC_ROOT: Final = Path(__file__).resolve().parents[2] / "fixtures" / "static"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

BASE_PHASE: Final = "base"
RECONCILE_STAGE: Final = "reconcile"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

#: Scenarios whose base phase is not a plain "deliver and reconcile" run, so driving
#: them here would exercise their harness rather than replay. Each is covered by its own
#: suite; replay's claim is about the log a run leaves behind, and every other scenario
#: leaves one the same way.
SKIP_SCENARIOS: Final[frozenset[str]] = frozenset({"assertions_contradiction"})


def config() -> Config:
    """The validated S6 document this session runs against (S7.1)."""
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    """The dbt var payload for one config, from S4.2's one generator."""
    payload, _ = blocking_rules_from_config(cfg)
    return payload


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def replayable_scenarios() -> list[str]:
    """Every committed scenario this file can drive, discovered rather than listed.

    AC1 names `base_10`, `merge_scenario`, `split_scenario` and `assertions_scenario`.
    `split_scenario` is not committed — ER-077 is blocked — so a hard-coded list would
    fail collection on a missing directory, and a list trimmed to what exists today
    would silently stop covering it once it lands. Discovery does neither.
    """
    return sorted(
        directory.name
        for directory in discover_scenarios(STATIC_ROOT)
        if directory.name not in SKIP_SCENARIOS and (directory / "scenario.yaml").is_file()
    )


@dataclass
class Dbt:
    """dbt as a stage invokes it: real `--vars`, no connection spanning it (S4.0b)."""

    connection: duckdb.DuckDBPyConnection
    artifacts: Path
    cfg: Config

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


@dataclass
class Replayed:
    """One scenario driven through one or more phases on a live lake."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    scenario: Scenario
    dbt: Dbt
    root: Path
    run_id: str = ""

    def phase(self, name: str) -> subprocess.CompletedProcess[str]:
        """Deliver, ingest, standardize, score and reconcile one phase under one run."""
        self.run_id = str(ULID())
        for source, path in self.scenario.inputs_for(name).items():
            directory = self.root / name / source
            directory.mkdir(parents=True, exist_ok=True)
            (directory / path.name).write_bytes(path.read_bytes())
        for source in self.scenario.inputs_for(name):
            result = run_er(
                "ingest",
                "--source",
                source,
                "--path",
                str(self.root / name),
                "--run-id",
                self.run_id,
                "--json",
            )
            assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
        self.dbt.standardize()
        scored = run_er("match", "--mode", MODE_FULL, "--run-id", self.run_id, "--json")
        assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr
        return run_er("reconcile", "--run-id", self.run_id)


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return config()


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored."""
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
def lake(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[tuple[duckdb.DuckDBPyConnection, Dbt, Path]]:
    """An initialised lake with the committed model published and active."""
    connection = initialised_lake
    dbt = Dbt(connection=connection, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")

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
        yield connection, dbt, tmp_path / "drop"
    finally:
        connection.execute(f'USE "{database}".{schema}')


@pytest.mark.parametrize("scenario_name", replayable_scenarios())
def test_replay_reproduces_membership_on_every_scenario(
    lake: tuple[duckdb.DuckDBPyConnection, Dbt, Path], cfg: Config, scenario_name: str
) -> None:
    """AC1: folding the log yields exactly `entity_membership`, in both directions."""
    connection, dbt, root = lake
    scenario = load_scenario(scenario_name)
    replay = Replayed(connection=connection, cfg=cfg, scenario=scenario, dbt=dbt, root=root)
    for phase in scenario.phases:
        reconciled = replay.phase(phase)
        assert reconciled.returncode in (int(ExitCode.SUCCESS), int(ExitCode.NOTHING_TO_DO)), (
            f"{scenario_name}/{phase} reconcile exited {reconciled.returncode}\n"
            f"{reconciled.stdout}\n{reconciled.stderr}"
        )
        folded = assert_replay_reproduces_membership(connection)
        assert folded, f"{scenario_name}/{phase} produced an empty membership to replay"


def test_seq_dense_and_idempotency_key_unique(
    lake: tuple[duckdb.DuckDBPyConnection, Dbt, Path], cfg: Config
) -> None:
    """AC3: dense 1-based `seq` per run, and a unique idempotency key (S5.0, MINOR-event_id)."""
    connection, dbt, root = lake
    replay = Replayed(
        connection=connection, cfg=cfg, scenario=load_scenario("base_10"), dbt=dbt, root=root
    )
    reconciled = replay.phase(BASE_PHASE)
    assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr

    per_run = connection.execute(
        f"SELECT run_id, count(*), min(seq), max(seq), count(DISTINCT seq) "
        f"FROM {EVENTS} GROUP BY run_id ORDER BY run_id"
    ).fetchall()
    assert per_run, "no events were written at all, so density is vacuous"
    for run_id, total, lowest, highest, distinct in per_run:
        assert (int(lowest), int(highest), int(distinct)) == (1, int(total), int(total)), (
            f"run {run_id} has {total} event(s) with seq in [{lowest}, {highest}] and "
            f"{distinct} distinct value(s); replay orders by (occurred_at, seq) and a "
            "gap is indistinguishable from a lost event (S4.5.3)"
        )

    duplicated = connection.execute(
        f"SELECT run_id, entity_id, event_type, details_hash, count(*) FROM {EVENTS} "
        "GROUP BY ALL HAVING count(*) > 1"
    ).fetchall()
    assert not duplicated, (
        f"{len(duplicated)} idempotency key(s) appear more than once: {duplicated[:5]}. "
        "S4.5.4 writes an event at most once per (run_id, entity_id, event_type, "
        "details_hash)."
    )


def test_unchanged_rerun_emits_zero_events(
    lake: tuple[duckdb.DuckDBPyConnection, Dbt, Path], cfg: Config
) -> None:
    """AC4: a re-run over an unchanged edge set writes nothing, mints nothing, exits 10."""
    connection, dbt, root = lake
    replay = Replayed(
        connection=connection, cfg=cfg, scenario=load_scenario("base_10"), dbt=dbt, root=root
    )
    reconciled = replay.phase(BASE_PHASE)
    assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr

    before_events = int(scalar(connection, f"SELECT count(*) FROM {EVENTS}"))
    before_membership = connection.execute(
        f"SELECT record_key, entity_id FROM {MEMBERSHIP} ORDER BY record_key"
    ).fetchall()
    before_entities = int(scalar(connection, f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.entities"))
    assert before_events and before_membership, "the first run wrote nothing to re-run over"

    # Same corpus, same edge set, same P_old — only the run_id differs.
    again = run_er("reconcile", "--run-id", str(ULID()))
    assert again.returncode == int(ExitCode.NOTHING_TO_DO), (
        f"an unchanged re-run exited {again.returncode}, not "
        f"{int(ExitCode.NOTHING_TO_DO)}. A run that wrote no events because it FAILED "
        f"would satisfy the row counts below, which is why the code is asserted too.\n"
        f"{again.stdout}\n{again.stderr}"
    )
    assert int(scalar(connection, f"SELECT count(*) FROM {EVENTS}")) == before_events, (
        "the unchanged re-run appended events; S4.5.4 says a re-run producing identical "
        "output writes zero"
    )
    assert (
        int(scalar(connection, f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.entities"))
        == before_entities
    ), "the unchanged re-run minted a ULID"
    assert (
        connection.execute(
            f"SELECT record_key, entity_id FROM {MEMBERSHIP} ORDER BY record_key"
        ).fetchall()
        == before_membership
    ), "the unchanged re-run moved a record between entities"

    assert_replay_reproduces_membership(connection)


def test_point_in_time_replay_matches_time_travelled_membership(
    lake: tuple[duckdb.DuckDBPyConnection, Dbt, Path], cfg: Config
) -> None:
    """AC5: replay up to run N equals the table AS IT WAS at run N's snapshot.

    The strongest form of the claim: the log reconstructs a PAST state, not merely the
    present one. `supersession_scenario` is driven because its two phases genuinely
    change membership — a scenario whose batch phase is a no-op would compare two
    identical states and prove nothing.
    """
    connection, dbt, root = lake
    scenario = load_scenario("supersession_scenario")
    replay = Replayed(connection=connection, cfg=cfg, scenario=scenario, dbt=dbt, root=root)

    reconciled = replay.phase(BASE_PHASE)
    assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr
    first_run = replay.run_id

    boundary = connection.execute(
        f"SELECT max(occurred_at), max(seq) FROM {EVENTS} WHERE run_id = ?", [first_run]
    ).fetchone()
    assert boundary is not None and boundary[0] is not None, "run 1 wrote no events"
    up_to = (boundary[0], int(boundary[1]))

    # The snapshot version is READ from run_stages at runtime; S8.1 forbids an absolute
    # version in the source, and one would be wrong on the next run anyway.
    snapshot = scalar(
        connection,
        f"SELECT snapshot_end FROM {RUN_STAGES} WHERE run_id = ? AND stage = ?",
        first_run,
        RECONCILE_STAGE,
    )
    assert snapshot is not None, (
        f"run_stages has no snapshot_end for ({first_run}, {RECONCILE_STAGE}); the "
        "point-in-time read has nothing to travel to"
    )
    version = int(snapshot)

    # A second phase that genuinely moves records, so "then" and "now" differ.
    second = replay.phase(scenario.phases[1])
    assert second.returncode == int(ExitCode.SUCCESS), second.stdout + second.stderr

    now = connection.execute(
        f"SELECT record_key, entity_id FROM {MEMBERSHIP} ORDER BY record_key"
    ).fetchall()
    # DuckDB takes no bind parameter in AT (VERSION => …), so the value is proven to be
    # an integer above and interpolated rather than formatted in blind.
    then = connection.execute(
        f"SELECT record_key, entity_id FROM {MEMBERSHIP} AT (VERSION => {version}) "
        "ORDER BY record_key"
    ).fetchall()
    assert then != now, (
        "the time-travelled read equals the current one, so the second phase changed "
        "nothing and the point-in-time claim would hold trivially"
    )

    assert_replay_reproduces_membership(
        connection,
        up_to=up_to,
        expected={str(record): str(entity) for record, entity in then},
    )
