"""`er reconcile` commits the plan: membership, entities, events (S4.5.3, D3).

ER-073 proved the reconciler is a correct pure function. This module proves the stage
around it writes what the plan says, once, in the shape S5 declares — and that is a
different claim. Four properties are asserted here that no unit test can reach:

* **Membership is current state.** Exactly one `entity_membership` row per
  `(source_system, source_record_id)`, written only by `MERGE INTO` (D3). A merge
  loser's rows are rewritten to the survivor rather than deleted and re-inserted, so
  there is no instant at which a record belongs to a `merged` entity.
* **An unchanged re-run is a true no-op.** S4.0's ``10``, zero events, zero mints, and
  every `assigned_at` unmoved. That is what ER-080's replay fold and T-IDEM-1 both rest
  on, and it is only observable across two runs.
* **Events are dense and idempotent.** `seq` is 1-based and gap-free per `run_id`
  because replay orders by `(occurred_at, seq)` and a gap is indistinguishable from a
  lost event; the `(run_id, entity_id, event_type, details_hash)` key admits one row
  however many times a plan is applied.
* **The stage reports itself.** S5.2's promoted `entities_*` columns and the S4.5.6
  counters JSON, which is what an operator reads when a reconcile does something
  surprising.

The merge arm uses `incremental_batch` rather than `base_10`, because a merge needs a
batch that bridges two existing entities and `base_10` has no `batch/` phase — its ten
personas are exactly ten entities, so no truthful bridge exists in it (the scenario
manifest says so explicitly).

Scoring is done in process and reconciling through the real console script. That split
is deliberate: `er match` would need the committed model published to the object store
to be loadable through `model_registry.params_path`, which says nothing about
reconciliation, while `er reconcile` reads only the registry — so the stage under test
is exercised exactly as an operator invokes it, `run_stages` row and all.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.invariants import assert_membership_equals_components
from helpers.model import fixture_settings, load_fixture_model
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.api import assert_no_splink_relations_in_lake
from er.matching.full import score_full
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.obs.counters import DECLARED_COUNTERS, StageCounters
from er.obs.runctx import StageRun

BASE_SCENARIO: Final = "base_10"
MERGE_SCENARIO: Final = "incremental_batch"
BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
ENTITIES: Final = f"{SCHEMA_QUALIFIER}.entities"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"

RECONCILE_STAGE: Final = "reconcile"
MATCH_STAGE: Final = "match"

#: The S4.5.6 counter names AC6 requires in the JSON payload.
REQUIRED_COUNTERS: Final[tuple[str, ...]] = (
    "affected_entities",
    "affected_edges",
    "label_prop_iterations",
    "clusters_out",
    "members_added",
    "members_removed",
    "events_emitted",
)

#: The pair `incremental_batch`'s manifest declares as its bridge (`bridged_personas:
#: [P6, P7]`). Named here rather than derived so the arm fails loudly if the fixture
#: stops containing it, instead of silently asserting over some other pair.
BRIDGE_PAIR: Final[tuple[str, str]] = ("crm:C010", "webforms:W006")

#: Who the bridging assertion is attributed to in `assertions.created_by`.
STEWARD: Final = "er-074-bridge"

#: The four promoted `run_stages` columns S5.2 gives this stage.
PROMOTED_COLUMNS: Final[tuple[str, ...]] = (
    "entities_created",
    "entities_merged",
    "entities_split",
    "entities_retired",
)


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


def deliver(scenario: Scenario, phase: str, root: Path) -> Path:
    """Materialise one phase as the drop-folder root `er ingest --path` reads."""
    for source, path in scenario.inputs_for(phase).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def membership_rows(connection: duckdb.DuckDBPyConnection) -> list[tuple[Any, ...]]:
    """Every membership row, in a stable order — the snapshot AC2 and AC3 compare."""
    return connection.execute(
        f"SELECT source_system, source_record_id, record_key, entity_id, assigned_at "
        f"FROM {MEMBERSHIP} ORDER BY source_system, source_record_id"
    ).fetchall()


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
        """The selection `er standardize` runs: staging, then intermediate (S4.2)."""
        self("build", select=STAGING_SELECTOR)
        self("build", select=INTERMEDIATE_SELECTOR)

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


@dataclass
class Lake:
    """A namespace with a scenario ingested, standardized and scored."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    scenario: Scenario
    dbt: Dbt
    root: Path
    model: tuple[str, str]
    #: The run the most recent delivery was ingested under. S4.5.1's batch arm is
    #: scoped by `run_id` through `ingest_batches`, so a reconcile that wants to see a
    #: delivery must run under the SAME run — which is exactly what `er run-all` does
    #: when it chains ingest and reconcile. `runs` supports a supplied `--run-id` and
    #: re-opens the row rather than appending a second one.
    run_id: str = ""

    def ingest(self, phase: str) -> str:
        """Deliver and ingest one phase under a fresh run, then standardize."""
        self.run_id = str(ULID())
        delivery = deliver(self.scenario, phase, self.root / phase)
        for source in self.scenario.inputs_for(phase):
            result = run_er(
                "ingest", "--source", source, "--path", str(delivery), "--run-id", self.run_id
            )
            assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
        self.dbt.standardize()
        return self.run_id

    def score(self) -> None:
        """One in-process `er match --mode full` at the committed model."""
        model_version, tf_snapshot_id = self.model
        score_full(
            self.connection,
            self.cfg,
            StageRun(
                run_id=str(ULID()),
                stage=MATCH_STAGE,
                seq=1,
                started_at=datetime.now(UTC),
                counters=StageCounters(DECLARED_COUNTERS[MATCH_STAGE]),
            ),
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            settings=fixture_settings(),
        )

    def reconcile(self, run_id: str | None = None) -> subprocess.CompletedProcess[str]:
        """One `er reconcile` through the real console script.

        Defaults to the run the last delivery was ingested under, which is what makes
        the batch arm non-empty (S4.5.1). Passing an explicit ``run_id`` is how the
        unchanged-re-run arm asks for a run that delivered nothing.
        """
        chosen = self.run_id if run_id is None else run_id
        return run_er("reconcile", "--json", "--run-id", chosen)


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


def _build(
    scenario_name: str,
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Lake:
    """A lake with ``scenario_name``'s base phase ingested, standardized and scored."""
    scenario = load_scenario(scenario_name)
    dbt = Dbt(connection=connection, artifacts=tmp_path / "artifacts", cfg=cfg)
    lake = Lake(
        connection=connection,
        cfg=cfg,
        scenario=scenario,
        dbt=dbt,
        root=tmp_path / "drop",
        model=("", ""),
    )
    dbt("seed")
    lake.ingest(BASE_PHASE)
    model_version, tf_snapshot_id, _ = load_fixture_model(connection)
    lake.model = (model_version, tf_snapshot_id)
    lake.score()
    return lake


@pytest.fixture
def base_lake(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[Lake]:
    """`base_10` scored and ready to reconcile."""
    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        yield _build(BASE_SCENARIO, initialised_lake, cfg, tmp_path)
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


@pytest.fixture
def bridging_lake(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[Lake]:
    """`incremental_batch`'s base phase scored; its `batch/` bridges two entities."""
    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        yield _build(MERGE_SCENARIO, initialised_lake, cfg, tmp_path)
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def test_membership_is_one_row_per_record(base_lake: Lake) -> None:
    """AC1/AC7: every standardized record holds exactly one membership row."""
    result = base_lake.reconcile()
    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    records = int(scalar(base_lake.connection, f"SELECT count(*) FROM {STD_RECORDS}"))
    members = int(scalar(base_lake.connection, f"SELECT count(*) FROM {MEMBERSHIP}"))
    assert members == records, (
        f"{members} membership row(s) for {records} standardized record(s); D3 makes "
        "entity_membership current state, one row per record"
    )
    duplicates = base_lake.connection.execute(
        f"SELECT source_system, source_record_id, count(*) FROM {MEMBERSHIP} "
        "GROUP BY source_system, source_record_id HAVING count(*) > 1"
    ).fetchall()
    assert not duplicates, f"{len(duplicates)} record(s) hold more than one row: {duplicates}"

    # Every membership row names an entity that exists and is active.
    orphans = base_lake.connection.execute(
        f"SELECT m.record_key FROM {MEMBERSHIP} AS m LEFT JOIN {ENTITIES} AS e "
        "ON e.entity_id = m.entity_id WHERE e.entity_id IS NULL OR e.status <> 'active'"
    ).fetchall()
    assert not orphans, f"membership rows naming a non-active entity: {orphans}"

    assert_no_splink_relations_in_lake(base_lake.connection)
    assert_membership_equals_components(base_lake.connection)


def test_unchanged_rerun_emits_no_events_and_exits_10(base_lake: Lake) -> None:
    """AC2: a second reconcile with nothing new is S4.0's `10` and writes nothing."""
    first = base_lake.reconcile()
    assert first.returncode == int(ExitCode.SUCCESS), first.stdout + first.stderr
    before = membership_rows(base_lake.connection)
    events_before = int(scalar(base_lake.connection, f"SELECT count(*) FROM {EVENTS}"))
    entities_before = int(scalar(base_lake.connection, f"SELECT count(*) FROM {ENTITIES}"))
    assert before and events_before, "the first reconcile wrote nothing to compare against"

    # A fresh run that delivered nothing: no ingest_batches row, so S4.5.1's batch arm
    # is empty and the four delta arms have nothing since the watermark the first
    # reconcile just set.
    second = base_lake.reconcile(run_id=str(ULID()))
    assert second.returncode == int(ExitCode.NOTHING_TO_DO), (
        f"an unchanged re-run exited {second.returncode}, not "
        f"{int(ExitCode.NOTHING_TO_DO)}:\n{second.stdout}\n{second.stderr}"
    )

    assert membership_rows(base_lake.connection) == before, (
        "the unchanged re-run moved a membership row; `assigned_at` and `entity_id` "
        "must both be untouched, or T-IDEM-1 and ER-080's replay fold both fail"
    )
    assert int(scalar(base_lake.connection, f"SELECT count(*) FROM {EVENTS}")) == events_before
    assert (
        int(scalar(base_lake.connection, f"SELECT count(*) FROM {ENTITIES}")) == entities_before
    ), "the unchanged re-run minted an entity; zero mints is the claim (S4.5.3)"


def test_merge_rewrites_loser_membership_in_one_snapshot(bridging_lake: Lake) -> None:
    """AC4: a merge moves the loser's members to the survivor and retires its id.

    **Where the bridging edge comes from, and why it is asserted rather than scored.**
    `incremental_batch`'s manifest declares `crm:C010` as a bridge joining the entities
    of personas P6 and P7, and under the committed model it does not: C010's only edge
    at or above `auto_merge` is to `crm:C006` (P6) at 0.999991, and nothing reaches P7's
    `billing:B006` / `webforms:W006` — despite C010 carrying P7's exact email. The three
    comparisons that would carry that edge (`email`, `birth_date`, `addr_postal`) are
    the three `model_test_v1` reports as "m values not fully trained", which is the same
    model weakness ER-060 documented on `base_10`. S8.2's dedicated `merge_scenario` is
    ER-075's and does not exist yet, and this ticket's own scope defers it.

    So the bridge is supplied as an `always` assertion over the pair the fixture already
    names. That is a first-class way to produce one: S4.4's adjustment injects an active
    `always` into the clustering edge set at `p = 1.0`, and AC4's subject is what the
    apply path does with a merge — the loser's rows rewritten, `status='merged'`,
    `merged_into` set, zero rows left referencing the loser — not the provenance of the
    edge that caused it. Asserting the bridge also makes the arm independent of a model
    score that a refit could move.

    The fixture defect is real and is reported separately; it is not patched here,
    because `fixtures/static/incremental_batch/` is ER-064's.
    """
    first = bridging_lake.reconcile()
    assert first.returncode == int(ExitCode.SUCCESS), first.stdout + first.stderr
    before = {
        str(key): str(entity)
        for key, entity in bridging_lake.connection.execute(
            f"SELECT record_key, entity_id FROM {MEMBERSHIP}"
        ).fetchall()
    }
    assert before, "the base reconcile wrote no membership"

    bridging_lake.ingest(BATCH_PHASE)
    bridging_lake.score()

    # The pair the manifest declares, in two different entities after the base run.
    left, right = BRIDGE_PAIR
    asserted = run_er(
        "assert", "add", "--kind", "always", "--a", left, "--b", right, "--by", STEWARD
    )
    assert asserted.returncode == int(ExitCode.SUCCESS), asserted.stdout + asserted.stderr

    second = bridging_lake.reconcile()
    assert second.returncode == int(ExitCode.SUCCESS), second.stdout + second.stderr

    merged = bridging_lake.connection.execute(
        f"SELECT entity_id, merged_into FROM {ENTITIES} WHERE status = 'merged'"
    ).fetchall()
    assert merged, (
        "the bridging always assertion produced no merged entity.\n"
        f"  reconcile manifest: {second.stdout.strip()}\n"
        f"  entity statuses: "
        + str(
            bridging_lake.connection.execute(
                f"SELECT status, count(*) FROM {ENTITIES} GROUP BY status ORDER BY status"
            ).fetchall()
        )
    )

    for loser, survivor in merged:
        assert survivor is not None, f"{loser} is merged with a NULL merged_into (S5)"
        holding = int(
            scalar(
                bridging_lake.connection,
                f"SELECT count(*) FROM {MEMBERSHIP} WHERE entity_id = ?",
                loser,
            )
        )
        assert holding == 0, (
            f"{holding} membership row(s) still name the merged entity {loser}; the "
            "loser's rows are rewritten to the survivor in the same statement (D3)"
        )
        survivor_status = scalar(
            bridging_lake.connection,
            f"SELECT status FROM {ENTITIES} WHERE entity_id = ?",
            survivor,
        )
        assert str(survivor_status) == "active", (
            f"the survivor {survivor} is {survivor_status!r}; a merge target holds "
            "members and must be active (S4.5.3)"
        )

    # Both endpoints of the asserted bridge now sit in one entity, which is the merge
    # actually having happened rather than an unrelated pair having merged.
    placement = dict(
        bridging_lake.connection.execute(
            f"SELECT record_key, entity_id FROM {MEMBERSHIP} WHERE record_key IN (?, ?)",
            [left, right],
        ).fetchall()
    )
    assert placement.get(left) == placement.get(right), (
        f"{left} and {right} carry an active always assertion but sit in "
        f"{placement}; S4.4 injects the edge at p = 1.0 and clustering must honour it"
    )

    assert_membership_equals_components(bridging_lake.connection)


def test_event_seq_is_dense_and_idempotency_key_unique(base_lake: Lake) -> None:
    """AC5: `seq` is dense and 1-based per run, and the idempotency key admits one row."""
    run_id = base_lake.run_id
    result = base_lake.reconcile()
    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    seqs = [
        int(row[0])
        for row in base_lake.connection.execute(
            f"SELECT seq FROM {EVENTS} WHERE run_id = ? ORDER BY seq", [run_id]
        ).fetchall()
    ]
    assert seqs, "the reconcile emitted no events; density would be vacuous"
    assert seqs == list(range(1, len(seqs) + 1)), (
        f"seq is not dense and 1-based: {seqs[:20]}. Replay orders by (occurred_at, "
        "seq), so a gap is indistinguishable from a lost event (S4.5.3)"
    )

    duplicates = base_lake.connection.execute(
        f"SELECT run_id, entity_id, event_type, details_hash, count(*) FROM {EVENTS} "
        "GROUP BY run_id, entity_id, event_type, details_hash HAVING count(*) > 1"
    ).fetchall()
    assert not duplicates, (
        f"{len(duplicates)} idempotency key(s) hold more than one row: {duplicates[:5]}"
    )


def test_run_stages_counters_populated(base_lake: Lake) -> None:
    """AC6: the four promoted columns and the S4.5.6 counters JSON are written."""
    run_id = base_lake.run_id
    result = base_lake.reconcile()
    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    columns = (*PROMOTED_COLUMNS, "counters", "snapshot_start", "snapshot_end")
    rows = base_lake.connection.execute(
        f"SELECT {', '.join(columns)} FROM {RUN_STAGES} WHERE run_id = ? AND stage = ?",
        [run_id, RECONCILE_STAGE],
    ).fetchall()
    assert len(rows) == 1, f"{len(rows)} run_stages rows for {RECONCILE_STAGE!r}; S5.2 writes one"
    recorded = dict(zip(columns, rows[0], strict=True))

    for column in PROMOTED_COLUMNS:
        assert recorded[column] is not None, (
            f"run_stages.{column} is NULL; S5.2 promotes it for this stage and the "
            "reconcile reported it"
        )
    counters = json.loads(str(recorded["counters"]))
    for name in REQUIRED_COUNTERS:
        assert counters.get(name) is not None, f"counters JSON is missing {name} (S4.5.6)"

    # A recorded range, with no assertion about how many snapshots it spans: the count
    # is DuckLake's business and would make this test a statement about the engine.
    assert recorded["snapshot_start"] is not None
    assert recorded["snapshot_end"] is not None
