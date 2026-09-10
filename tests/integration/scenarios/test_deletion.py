"""T-DEL-1: the S4.5.5 retraction path, end to end (S4.1.1, S4.2, S4.5.1, S4.5.3).

`deletion_scenario`, phases `base -> refresh -> resurrect`. The `refresh/` delivery
is the one `--full-refresh-keys` ingest and omits exactly two live keys — the bridge
record of a five-member entity and a singleton — so one refresh reaches every
deletion outcome the spec names: tombstone rows with the sentinel `content_hash`,
permanent edge invalidation, `member_removed` on each prior entity, a `split` for
the fragment the bridge removal disconnects, and `retired` for the entity that
empties. The `resurrect/` delivery then re-appears the bridge — every compared value
identical, a fresh submission timestamp, because `raw_records`' logical key
includes `content_hash` and byte-identical bytes could not append — and it must
re-enter as an ordinary new record — scored, re-clustered, re-merging the two
fragments under S4.5.3's claimant rule — with no special case anywhere (S4.1.1).

The expected files were generated from real runs under `ER_REGEN_DELETION=1` and are
compared, never authored to match (S8.2.1). Regeneration rewrites them into the
mounted artifacts directory and then fails deliberately, so it can never leave a
green suite.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.compare import assert_ids_stable, assert_partition_equal
from helpers.invariants import assert_membership_equals_components
from helpers.model import load_fixture_model
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import (
    ARTIFACTS_DIR,
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

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
FIXTURE_ROOT: Final = REPO_ROOT / "fixtures" / "static"

SCENARIO_NAME: Final = "deletion_scenario"

BASE_PHASE: Final = "base"
REFRESH_PHASE: Final = "refresh"
RESURRECT_PHASE: Final = "resurrect"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
ENTITIES: Final = f"{SCHEMA_QUALIFIER}.entities"
RAW_RECORDS: Final = f"{SCHEMA_QUALIFIER}.raw_records"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"
INGEST_BATCHES: Final = f"{SCHEMA_QUALIFIER}.ingest_batches"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

#: S4.1.1's tombstone sentinel: sixty-four zeros where a content hash would be.
TOMBSTONE_HASH: Final = "0" * 64

RETIRED_STATUS: Final = "retired"

#: The designed structure, spelled here so a test reads as a claim about the fixture.
#: The fixture lint derives the same structure from the CSVs (AC6).
X_SIDE: Final[tuple[str, ...]] = ("billing:B321", "crm:C321")
Y_SIDE: Final[tuple[str, ...]] = ("crm:C322", "webforms:W322")
BRIDGE: Final = "webforms:W321"
SINGLETON: Final = "crm:C323"

#: Set to rewrite the committed expectations from a real run; the test then fails.
REGEN_ENV: Final = "ER_REGEN_DELETION"


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


def membership_pairs(connection: duckdb.DuckDBPyConnection) -> list[tuple[str, str]]:
    """`(record_key, entity_id)` for every membership row."""
    return [
        (str(record), str(entity))
        for record, entity in connection.execute(
            f"SELECT record_key, entity_id FROM {MEMBERSHIP} ORDER BY record_key"
        ).fetchall()
    ]


def label_map(pairs: list[tuple[str, str]]) -> dict[str, str]:
    """`entity_label -> entity_id`, allocated by ascending minimum member record_key."""
    groups: dict[str, list[str]] = {}
    for record, entity in pairs:
        groups.setdefault(entity, []).append(record)
    return {
        f"E{index + 1}": entity
        for index, entity in enumerate(sorted(groups, key=lambda entity: min(groups[entity])))
    }


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
class Deletion:
    """The scenario driven through its phases, holding the base label map."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    scenario: Scenario
    dbt: Dbt
    root: Path
    run_id: str = ""
    base_labels: dict[str, str] = field(default_factory=dict)
    base_entity_ids: frozenset[str] = frozenset()

    @property
    def expected_dir(self) -> Path:
        return FIXTURE_ROOT / SCENARIO_NAME / "expected"

    def delivery_root(self, phase: str) -> Path:
        return self.root / phase

    def phase(self, name: str) -> None:
        """Deliver, ingest, standardize, score and reconcile one phase under one run.

        The refresh phase's ingest carries ``--full-refresh-keys`` — S8.2.1 makes it
        the ONLY phase that does, because a full-refresh delivery tombstones every
        key it omits and the resurrection must not.
        """
        self.run_id = str(ULID())
        delivery = deliver(self.scenario, name, self.delivery_root(name))
        refresh = ("--full-refresh-keys",) if name == REFRESH_PHASE else ()
        for source in self.scenario.inputs_for(name):
            result = run_er(
                "ingest",
                "--source",
                source,
                "--path",
                str(delivery),
                *refresh,
                "--run-id",
                self.run_id,
                "--json",
            )
            # `10` is a legitimate per-source outcome mid-scenario: the refresh
            # delivery re-states billing's whole key set unchanged, and an ingest
            # with nothing new, nothing changed and nothing tombstoned is S4.0's
            # nothing-to-do, not a failure.
            assert result.returncode in (
                int(ExitCode.SUCCESS),
                int(ExitCode.NOTHING_TO_DO),
            ), result.stdout + result.stderr
        self.dbt.standardize()
        scored = run_er("match", "--mode", MODE_FULL, "--run-id", self.run_id, "--json")
        assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr
        reconciled = run_er("reconcile", "--run-id", self.run_id)
        assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr

    def entity_of(self, record: str) -> str:
        return str(
            scalar(
                self.connection,
                f"SELECT entity_id FROM {MEMBERSHIP} WHERE record_key = ?",
                record,
            )
        )

    def events_of_run(self) -> list[tuple[str, str, int]]:
        """`(entity_id, event_type, count)` for this run, in query order."""
        return [
            (str(entity), str(event_type), int(count))
            for entity, event_type, count in self.connection.execute(
                f"SELECT entity_id, event_type, count(*) FROM {EVENTS} WHERE run_id = ? "
                "GROUP BY entity_id, event_type ORDER BY entity_id, event_type",
                [self.run_id],
            ).fetchall()
        ]


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
def deletion(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Deletion]:
    """`deletion_scenario` with its base phase applied."""
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (BASE_PHASE, REFRESH_PHASE, RESURRECT_PHASE)
    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")

    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        model_version, _, settings = load_fixture_model(initialised_lake)
        published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
        object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
        initialised_lake.execute(
            f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
            [published, model_version],
        )
        driven = Deletion(
            connection=initialised_lake,
            cfg=cfg,
            scenario=scenario,
            dbt=dbt,
            root=tmp_path / "drop",
        )
        driven.phase(BASE_PHASE)
        pairs = membership_pairs(initialised_lake)
        driven.base_labels = label_map(pairs)
        driven.base_entity_ids = frozenset(entity for _, entity in pairs)
        if os.environ.get(REGEN_ENV):
            _regenerate(driven, BASE_PHASE)
        yield driven
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def _persona_ids(scenario: Scenario, phase: str) -> dict[str, str]:
    """`record_key -> persona_id` from the delivered CSVs of ``phase`` (S8.2.1)."""
    labels: dict[str, str] = {}
    for source, path in scenario.inputs_for(phase).items():
        for line in path.read_text(encoding="utf-8").splitlines()[1:]:
            if line:
                fields = line.split(",")
                labels[f"{source}:{fields[0]}"] = fields[-1]
    return labels


def _labels_after(deletion: Deletion) -> dict[str, str]:
    """The base map, extended with labels for entities minted since (S8.2.1 order)."""
    pairs = membership_pairs(deletion.connection)
    groups: dict[str, list[str]] = {}
    for record, entity in pairs:
        groups.setdefault(entity, []).append(record)
    by_id = {entity: label for label, entity in deletion.base_labels.items()}
    novel = sorted(
        (entity for entity in groups if entity not in by_id),
        key=lambda entity: min(groups[entity]),
    )
    ordinal = len(by_id)
    labels = dict(deletion.base_labels)
    for entity in novel:
        ordinal += 1
        labels[f"E{ordinal}"] = entity
    return labels


def _regenerate(deletion: Deletion, phase: str) -> None:
    """Write this phase's expected files into the mounted artifacts directory.

    Labels are the BASE phase's map extended in minted order, so a later expectation
    names the retained fragment by the label it carried before the split — which is
    what makes the committed files show a split and a re-merge rather than a
    relabelling.
    """
    out = ARTIFACTS_DIR / SCENARIO_NAME / phase
    out.mkdir(parents=True, exist_ok=True)

    pairs = membership_pairs(deletion.connection)
    labels = _labels_after(deletion) if phase != BASE_PHASE else label_map(pairs)
    by_id = {entity: label for label, entity in labels.items()}
    personas: dict[str, str] = {}
    for earlier in deletion.scenario.phases:
        personas.update(_persona_ids(deletion.scenario, earlier))

    rows = sorted(
        (personas.get(record, "\\N"), *record.split(":", 1), by_id.get(entity, entity))
        for record, entity in pairs
    )
    (out / "membership.csv").write_text(
        "persona_id,source_system,source_record_id,entity_label\n"
        + "".join(f"{p},{s},{r},{label}\n" for p, s, r, label in rows),
        encoding="utf-8",
    )
    counted = deletion.events_of_run()
    (out / "events.csv").write_text(
        "entity_label,event_type,count\n"
        + "".join(
            f"{label},{event_type},{count}\n"
            for label, event_type, count in sorted(
                (by_id.get(entity, entity), event_type, count)
                for entity, event_type, count in counted
            )
        ),
        encoding="utf-8",
    )


def _assert_events_match_expectation(deletion: Deletion, phase: str) -> None:
    """AC4/AC5's file half: `expected/<phase>/events.csv` is this run's events."""
    labels = _labels_after(deletion)
    by_id = {entity: label for label, entity in labels.items()}
    observed = sorted(
        (by_id.get(entity, entity), event_type, count)
        for entity, event_type, count in deletion.events_of_run()
    )
    committed_path = deletion.expected_dir / phase / "events.csv"
    committed = [
        (fields[0], fields[1], int(fields[2]))
        for fields in (
            line.split(",")
            for line in committed_path.read_text(encoding="utf-8").splitlines()[1:]
            if line
        )
    ]
    assert observed == committed, (
        f"{committed_path} does not encode this run's events.\n"
        f"  observed:  {observed}\n  committed: {committed}"
    )


def _run_refresh_and_assert_tombstones(deletion: Deletion) -> dict[str, str]:
    """The refresh phase, with AC1's ingest-and-standardize claims asserted inline.

    Returns the pre-refresh entity of each record, captured before anything moves.
    """
    before = dict(membership_pairs(deletion.connection))
    deletion.phase(REFRESH_PHASE)

    # AC1: two sentinel tombstone rows, and the refresh batches counted them.
    tombstones = deletion.connection.execute(
        f"SELECT source_system || ':' || source_record_id AS record_key, "
        f"content_hash, deleted_at, json_type(payload) FROM {RAW_RECORDS} "
        "WHERE is_deleted ORDER BY 1"
    ).fetchall()
    assert [(str(row[0])) for row in tombstones] == sorted([BRIDGE, SINGLETON])
    for record_key, content_hash, deleted_at, payload_type in tombstones:
        assert str(content_hash) == TOMBSTONE_HASH, f"{record_key}: {content_hash}"
        assert deleted_at is not None, f"{record_key}: deleted_at is NULL"
        # S4.1.1 says `payload = NULL` while S5 declares the column `JSON NOT NULL`;
        # ER-032 reconciles the two as the JSON null document, whose json_type is
        # 'NULL'. A SQL NULL here would violate S5.
        assert str(payload_type) == "NULL", (
            f"{record_key}: a tombstone carries the JSON null document, "
            f"got json_type {payload_type!r}"
        )
    counted = int(
        scalar(
            deletion.connection,
            f"SELECT coalesce(sum(tombstone_count), 0) FROM {INGEST_BATCHES} WHERE run_id = ?",
            deletion.run_id,
        )
    )
    assert counted == 2, f"ingest_batches.tombstone_count sums to {counted}, not 2"
    remaining = {
        str(row[0])
        for row in deletion.connection.execute(f"SELECT record_key FROM {STD_RECORDS}").fetchall()
    }
    assert not remaining & {BRIDGE, SINGLETON}, (
        f"tombstoned key(s) {remaining & {BRIDGE, SINGLETON}} still standardized (S4.2)"
    )
    return before


def test_deletion_retracts_edges_and_resurrection_restores_membership(
    deletion: Deletion,
) -> None:
    """T-DEL-1: AC1-AC5, AC7, AC8 across refresh and resurrect."""
    base = dict(membership_pairs(deletion.connection))
    assert set(base) == {*X_SIDE, BRIDGE, *Y_SIDE, SINGLETON}, f"base delivered {sorted(base)}"
    assert len({base[key] for key in (*X_SIDE, BRIDGE, *Y_SIDE)}) == 1, (
        f"the five Fontaine records are not one entity: {base}"
    )
    entity_before = base[X_SIDE[0]]

    if not os.environ.get(REGEN_ENV):
        assert_partition_equal(
            membership_pairs(deletion.connection),
            deletion.expected_dir / BASE_PHASE / "membership.csv",
        )
    assert_membership_equals_components(deletion.connection)

    _run_refresh_and_assert_tombstones(deletion)
    refresh_run = deletion.run_id

    # AC2: every incident row is invalidated in place, none inserted or deleted.
    incident = deletion.connection.execute(
        f"SELECT rec_a_key, rec_b_key, is_active, invalidated_run_id, count(*) OVER "
        f"(PARTITION BY rec_a_key, rec_b_key, model_version, tf_snapshot_id) AS per_key "
        f"FROM {MATCH_SCORES} WHERE rec_a_key IN (?, ?) OR rec_b_key IN (?, ?)",
        [BRIDGE, SINGLETON, BRIDGE, SINGLETON],
    ).fetchall()
    assert incident, "the bridge scored no edges in base; there was nothing to invalidate"
    for rec_a, rec_b, is_active, invalidated_run_id, per_key in incident:
        assert not is_active, f"({rec_a}, {rec_b}) is still active after the refresh"
        assert str(invalidated_run_id) == refresh_run, (
            f"({rec_a}, {rec_b}) was invalidated by {invalidated_run_id}, not the "
            f"refresh run {refresh_run}"
        )
        assert int(per_key) == 1, f"({rec_a}, {rec_b}) holds {per_key} rows for one key"

    if os.environ.get(REGEN_ENV):
        _regenerate(deletion, REFRESH_PHASE)
        deletion.base_labels = _labels_after(deletion)
        deletion.phase(RESURRECT_PHASE)
        _regenerate(deletion, RESURRECT_PHASE)
        pytest.fail(f"{REGEN_ENV} set: expectations rewritten under artifacts/, not asserted")

    # AC3: exactly one member_removed per TOMBSTONED record, on its prior entity,
    # distinguished by cause: the run also emits a `recluster`-cause removal on the
    # Fontaine entity for the fragment the split carries away, which is the split's
    # own account of itself (S4.5.3) and not a deletion event.
    by_cause = [
        (str(entity), str(cause), int(count))
        for entity, cause, count in deletion.connection.execute(
            f"SELECT entity_id, json_extract_string(details, '$.cause'), count(*) "
            f"FROM {EVENTS} WHERE run_id = ? AND event_type = 'member_removed' "
            "GROUP BY 1, 2 ORDER BY 1, 2",
            [deletion.run_id],
        ).fetchall()
    ]
    tombstoned = [(entity, count) for entity, cause, count in by_cause if cause == "tombstone"]
    assert sorted(tombstoned) == sorted([(entity_before, 1), (base[SINGLETON], 1)]), (
        f"tombstone-cause member_removed events are {tombstoned}, of {by_cause}"
    )
    events = deletion.events_of_run()
    splits = [(entity, count) for entity, kind, count in events if kind == "split"]
    assert len(splits) == 1 and splits[0][1] == 1, f"split events are {splits}"
    retirements = [(entity, count) for entity, kind, count in events if kind == "retired"]
    assert retirements == [(base[SINGLETON], 1)], f"retired events are {retirements}"

    # AC4: the refresh expectation holds, and the rank-1 fragment keeps the id.
    after_refresh = dict(membership_pairs(deletion.connection))
    assert {after_refresh[key] for key in X_SIDE} == {entity_before}, (
        "the fragment holding the smaller minimum record_key lost the entity_id"
    )
    minted = {after_refresh[key] for key in Y_SIDE}
    assert len(minted) == 1 and not minted & deletion.base_entity_ids
    assert_ids_stable(
        membership_pairs(deletion.connection),
        deletion.expected_dir / REFRESH_PHASE / "membership.csv",
        _labels_after(deletion),
    )
    _assert_events_match_expectation(deletion, REFRESH_PHASE)
    assert_membership_equals_components(deletion.connection)

    # The label map accumulates across phases: E3 is minted by the refresh and
    # merged away by the resurrection, and the resurrect expectation still names
    # its `merged` event by the label it carried while it held members.
    deletion.base_labels = _labels_after(deletion)

    # AC5: resurrection is an ordinary re-entry — scored, clustered, one row.
    deletion.phase(RESURRECT_PHASE)
    resurrected = int(
        scalar(
            deletion.connection,
            f"SELECT coalesce(sum(resurrected_count), 0) FROM {INGEST_BATCHES} WHERE run_id = ?",
            deletion.run_id,
        )
    )
    assert resurrected == 1, f"ingest_batches.resurrected_count sums to {resurrected}, not 1"
    hash_of_bridge = str(
        scalar(
            deletion.connection,
            f"SELECT content_hash FROM {STD_RECORDS} WHERE record_key = ?",
            BRIDGE,
        )
    )
    live_edges = deletion.connection.execute(
        f"SELECT rec_a_key, rec_b_key, rec_a_content_hash, rec_b_content_hash "
        f"FROM {MATCH_SCORES} WHERE is_active AND (rec_a_key = ? OR rec_b_key = ?)",
        [BRIDGE, BRIDGE],
    ).fetchall()
    assert live_edges, "the resurrected record was not re-scored"
    for rec_a, rec_b, hash_a, hash_b in live_edges:
        endpoint_hash = str(hash_a) if str(rec_a) == BRIDGE else str(hash_b)
        assert endpoint_hash == hash_of_bridge, (
            f"({rec_a}, {rec_b}) carries a stale endpoint hash for the resurrected record"
        )
    membership_rows = int(
        scalar(
            deletion.connection,
            f"SELECT count(*) FROM {MEMBERSHIP} WHERE record_key = ?",
            BRIDGE,
        )
    )
    assert membership_rows == 1
    assert deletion.entity_of(BRIDGE) == entity_before, (
        "the re-merge did not settle on the fragment holding the smaller minimum "
        "record_key (S4.5.3's claimant tiebreak)"
    )
    assert_ids_stable(
        membership_pairs(deletion.connection),
        deletion.expected_dir / RESURRECT_PHASE / "membership.csv",
        _labels_after(deletion),
    )
    _assert_events_match_expectation(deletion, RESURRECT_PHASE)
    assert_membership_equals_components(deletion.connection)

    # AC8: re-running the resurrect phase unchanged is a chain of `10`s.
    raw_before = int(scalar(deletion.connection, f"SELECT count(*) FROM {RAW_RECORDS}"))
    events_before = int(scalar(deletion.connection, f"SELECT count(*) FROM {EVENTS}"))
    for source in deletion.scenario.inputs_for(RESURRECT_PHASE):
        again = run_er(
            "ingest",
            "--source",
            source,
            "--path",
            str(deletion.delivery_root(RESURRECT_PHASE)),
            "--json",
        )
        assert again.returncode == int(ExitCode.NOTHING_TO_DO), again.stdout + again.stderr
    rerun_id = str(ULID())
    rerun = run_er("run-all", "--mode", "incremental", "--skip-ingest", "--run-id", rerun_id)
    assert rerun.returncode == int(ExitCode.SUCCESS), rerun.stdout + rerun.stderr
    # Per-stage exit codes live in the S5.2 stderr records — `run_stages` promotes
    # `status`, not the code — so the re-run's own stream is what is read.
    stage_codes = {
        str(record["stage"]): int(record["exit_code"])
        for line in (rerun.stderr + rerun.stdout).splitlines()
        if line.startswith("{")
        for record in [json.loads(line)]
        if record.get("run_id") == rerun_id and "exit_code" in record and "stage" in record
    }
    assert stage_codes and set(stage_codes.values()) == {int(ExitCode.NOTHING_TO_DO)}, (
        f"the unchanged re-run's stages reported {stage_codes}; S4.0 gives every one "
        "of them a nothing-to-do exit of 10"
    )
    assert int(scalar(deletion.connection, f"SELECT count(*) FROM {RAW_RECORDS}")) == raw_before
    assert int(scalar(deletion.connection, f"SELECT count(*) FROM {EVENTS}")) == events_before


def test_emptied_entity_is_retired_and_holds_no_members(deletion: Deletion) -> None:
    """AC3's retirement half, against `entities` rather than the event stream."""
    base = dict(membership_pairs(deletion.connection))
    singleton_entity = base[SINGLETON]

    _run_refresh_and_assert_tombstones(deletion)

    # No regeneration arm: every assertion below reads the live lake, not a
    # committed expectation, so the test is equally valid under ER_REGEN_DELETION.
    status = str(
        scalar(
            deletion.connection,
            f"SELECT status FROM {ENTITIES} WHERE entity_id = ?",
            singleton_entity,
        )
    )
    assert status == RETIRED_STATUS, (
        f"the emptied entity's status is {status!r}; tombstoning its only record must "
        f"retire it (S4.5.3, S4.5.5)"
    )
    members = int(
        scalar(
            deletion.connection,
            f"SELECT count(*) FROM {MEMBERSHIP} WHERE entity_id = ?",
            singleton_entity,
        )
    )
    assert members == 0, f"the retired entity still holds {members} membership row(s)"
    assert_membership_equals_components(deletion.connection)
