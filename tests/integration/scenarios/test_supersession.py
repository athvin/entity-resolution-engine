"""T-SUPER-1: a re-delivered record retires its old edges and leaves its entity (S4.5.5).

Supersession is the only construction in the suite that makes two things observable at
once, and nothing else exercises either:

* **INV-SCORE's endpoint-hash clause.** S4.3.3 makes `match_probability` a pure function
  of the two endpoint `content_hash`es among other things. A row scored before a
  re-delivery is therefore not *wrong* — it is about a pair that no longer exists — and
  the only way to see that distinction is to move one endpoint and watch the row retire
  rather than change.
* **The affected-set widening rule.** S4.5.1 widens from a seed record to its entity's
  full membership. Here the batch delivers exactly one record and the other two members
  appear in no `ingest_batches` row for the run, so if the widening did not happen the
  reconcile would simply not see them — and the entity they used to share would be left
  holding a member that had already gone.

**Append-only is asserted, not assumed.** `er ingest` over the batch must report
`new_count = 0` and `changed_count = 1`, and `raw_records` must afterwards hold *two*
rows for the key with two distinct `content_hash` values. An implementation that
overwrote the first version would satisfy every downstream assertion in this file and
violate S4.1.

The expected files are generated from a real run under `ER_REGEN_SUPERSESSION=1`, which
rewrites them and then fails deliberately — `std_hash` is a SHA-256 over the
standardized row, and which records co-cluster is a property of the committed model, so
neither can be authored by hand. That is the same discipline `parity_pairs.csv` is held
to (S8.2.1).
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
from helpers.compare import assert_partition_equal
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
from er.entities.retraction import stale_edge_rows
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.full import MODE_FULL
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
SCENARIO_NAME: Final = "supersession_scenario"
SCENARIO_DIR: Final = REPO_ROOT / "fixtures" / "static" / SCENARIO_NAME

BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"
RAW_RECORDS: Final = f"{SCHEMA_QUALIFIER}.raw_records"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"

MEMBER_REMOVED: Final = "member_removed"

#: The record the batch re-delivers, and its two co-members that appear in no batch.
SUPERSEDED: Final = "webforms:W401"
CO_MEMBERS: Final[tuple[str, str]] = ("billing:B401", "crm:C401")

#: Set to rewrite the committed expectations from a real run; the test then fails.
REGEN_ENV: Final = "ER_REGEN_SUPERSESSION"


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


def manifest_line(stdout: str, key: str) -> dict[str, Any]:
    """The `--json` manifest carrying ``key``, picked out of everything else on stdout."""
    for line in stdout.splitlines():
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict) and key in payload:
            return payload
    raise AssertionError(f"no manifest with {key!r} on stdout:\n{stdout}")


def membership_pairs(connection: duckdb.DuckDBPyConnection) -> list[tuple[str, str]]:
    """`(record_key, entity_id)` for every membership row."""
    return [
        (str(record), str(entity))
        for record, entity in connection.execute(
            f"SELECT record_key, entity_id FROM {MEMBERSHIP} ORDER BY record_key"
        ).fetchall()
    ]


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
class Sup:
    """The scenario driven through one phase at a time."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    scenario: Scenario
    dbt: Dbt
    root: Path
    ingest_stdout: str = ""
    run_id: str = ""

    def phase(self, name: str) -> None:
        """Deliver, ingest, standardize, score and reconcile one phase under one run."""
        self.run_id = str(ULID())
        delivery = deliver(self.scenario, name, self.root / name)
        outputs: list[str] = []
        for source in self.scenario.inputs_for(name):
            result = run_er(
                "ingest",
                "--source",
                source,
                "--path",
                str(delivery),
                "--run-id",
                self.run_id,
                "--json",
            )
            assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
            outputs.append(result.stdout)
        self.ingest_stdout = "\n".join(outputs)
        self.dbt.standardize()

        # `er match` rather than an in-process `score_full`, because S4.5.5 invalidation
        # is a property of the STAGE and not of the scorer: S4.0b permits the scoring
        # path one write to `match_scores`, so the retirement runs in
        # `er.cli._MatchStage.run` and a test that called `score_full` directly would
        # score the batch without ever retiring the superseded record's edges — which is
        # precisely what AC3 asserts happens.
        scored = run_er("match", "--mode", MODE_FULL, "--run-id", self.run_id, "--json")
        assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr

        reconciled = run_er("reconcile", "--run-id", self.run_id)
        assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr


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
def sup(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Sup]:
    """The scenario with its `base` phase applied, ready for `batch`."""
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (BASE_PHASE, BATCH_PHASE)
    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")

    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        # Writes the `model_registry` row `er match` selects as active (S4.3.2). The
        # helper points `params_path` at the committed file on disk, which is what an
        # in-process scorer reads; `er match` fetches it through the object store like
        # any other run, so the settings are published and the row repointed at the
        # `s3://` object. Without this the stage raises "not an s3:// URI" before it
        # ever reaches the invalidation this scenario exists to observe.
        model_version, _, settings = load_fixture_model(initialised_lake)
        published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
        object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
        initialised_lake.execute(
            f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
            [published, model_version],
        )
        sup = Sup(
            connection=initialised_lake,
            cfg=cfg,
            scenario=scenario,
            dbt=dbt,
            root=tmp_path / "drop",
        )
        sup.phase(BASE_PHASE)
        if os.environ.get(REGEN_ENV):
            # The base expectation has to be captured here: by the time a test has run
            # the batch phase, the state this file describes no longer exists.
            _regenerate(sup, BASE_PHASE)
        yield sup
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def _persona_ids(scenario: Scenario, phase: str) -> dict[str, str]:
    """`record_key -> persona_id` from the delivered CSVs of ``phase``.

    S8.2.1 gives `membership.csv` a `persona_id` column and every input row carries the
    truth label as its last field. Reading it back rather than writing the null token
    keeps the expectation self-describing: a reader can see which persona each record
    was supposed to belong to without opening the inputs.
    """
    labels: dict[str, str] = {}
    for source, path in scenario.inputs_for(phase).items():
        lines = path.read_text(encoding="utf-8").splitlines()
        for line in lines[1:]:
            if not line:
                continue
            fields = line.split(",")
            labels[f"{source}:{fields[0]}"] = fields[-1]
    return labels


def _regenerate(sup: Sup, phase: str) -> None:
    """Write this phase's expected files into the mounted artifacts directory.

    S7.1's image COPYs the repository rather than bind-mounting it, so a file written at
    the fixture path inside the container never reaches the host. Only `artifacts/` is
    mounted, so that is where a containerised regeneration is collected from.
    """
    out = ARTIFACTS_DIR / SCENARIO_NAME / phase
    out.mkdir(parents=True, exist_ok=True)

    pairs = membership_pairs(sup.connection)
    groups: dict[str, list[str]] = {}
    for record, entity in pairs:
        groups.setdefault(entity, []).append(record)
    labels = {
        entity: f"E{index + 1}"
        for index, entity in enumerate(sorted(groups, key=lambda entity: min(groups[entity])))
    }
    personas = dict(_persona_ids(sup.scenario, BASE_PHASE))
    if phase != BASE_PHASE:
        # A later phase's delivery supersedes the base one for the keys it re-delivers,
        # which is the whole subject of this scenario: `webforms:W401` is P1 in `base`
        # and P2 in `batch`, and the expectation has to say the version it describes.
        personas.update(_persona_ids(sup.scenario, phase))
    rows = sorted(
        (personas.get(record, "\\N"), *record.split(":", 1), labels[entity])
        for record, entity in pairs
    )
    (out / "membership.csv").write_text(
        "persona_id,source_system,source_record_id,entity_label\n"
        + "".join(
            f"{persona},{system},{record},{label}\n" for persona, system, record, label in rows
        ),
        encoding="utf-8",
    )

    counted = sup.connection.execute(
        f"SELECT entity_id, event_type, count(*) FROM {EVENTS} WHERE run_id = ? "
        "GROUP BY entity_id, event_type ORDER BY entity_id, event_type",
        [sup.run_id],
    ).fetchall()
    (out / "events.csv").write_text(
        "entity_label,event_type,count\n"
        + "".join(
            f"{labels.get(str(entity), str(entity))},{event_type},{count}\n"
            for entity, event_type, count in counted
        ),
        encoding="utf-8",
    )

    hashes = sup.connection.execute(
        f"SELECT source_system, source_record_id, content_hash FROM {STD_RECORDS} "
        "ORDER BY source_system, source_record_id"
    ).fetchall()
    (out / "std_hashes.csv").write_text(
        "source_system,source_record_id,std_hash\n"
        + "".join(f"{system},{record},{digest}\n" for system, record, digest in hashes),
        encoding="utf-8",
    )


def test_superseded_record_invalidates_edges_and_leaves_its_entity(sup: Sup) -> None:
    """T-SUPER-1 / AC1, AC2, AC3, AC6: the whole supersession path, end to end."""
    base_membership = dict(membership_pairs(sup.connection))
    assert set(base_membership) == {SUPERSEDED, *CO_MEMBERS}, (
        f"the base phase did not place all three records: {base_membership}"
    )
    prior_entity = base_membership[SUPERSEDED]
    assert len({base_membership[key] for key in base_membership}) == 1, (
        "the base phase did not form ONE entity; the supersession has nothing to leave"
    )
    prior_hash = str(
        scalar(
            sup.connection,
            f"SELECT content_hash FROM {STD_RECORDS} WHERE record_key = ?",
            SUPERSEDED,
        )
    )

    sup.phase(BATCH_PHASE)

    # AC1: append-only. A second version, not an overwrite.
    #
    # The ticket words this as `new_count` / `changed_count`; S4.1's manifest, as ER-031
    # implemented it, spells them `new` and `changed`. The semantics AC1 asks for are
    # exactly these fields, so the implemented names are the ones asserted — the same
    # rule this board applies wherever a ticket and INTERFACES.md disagree on a name.
    ingest = manifest_line(sup.ingest_stdout, "changed")
    assert ingest["new"] == 0, f"the batch reported new records: {ingest}"
    assert ingest["changed"] == 1, f"the batch did not report a change: {ingest}"
    assert ingest["unchanged"] == 0, f"the batch reported an unchanged record: {ingest}"
    versions = sup.connection.execute(
        f"SELECT count(*), count(DISTINCT content_hash) FROM {RAW_RECORDS} "
        "WHERE source_system = 'webforms' AND source_record_id = 'W401'"
    ).fetchone()
    assert versions == (2, 2), (
        f"raw_records holds {versions} for the superseded key; S4.1 is append-only, so "
        "the re-delivery adds a version and overwrites nothing"
    )

    # AC2: one current standardized row, carrying the NEW hash.
    current = sup.connection.execute(
        f"SELECT count(*), max(content_hash) FROM {STD_RECORDS} WHERE record_key = ?",
        [SUPERSEDED],
    ).fetchone()
    assert current is not None and current[0] == 1, "int_std_records holds two current rows"
    assert str(current[1]) != prior_hash, "the re-delivery did not change the content_hash"

    # AC3: edges scored against the prior hash are retired, in place.
    assert stale_edge_rows(sup.connection) == [], (
        "stale edges survived the batch run; invalidation runs before scoring (S4.5.5)"
    )
    retired = sup.connection.execute(
        f"SELECT rec_a_key, rec_b_key, invalidated_run_id FROM {MATCH_SCORES} "
        "WHERE NOT is_active ORDER BY rec_a_key, rec_b_key"
    ).fetchall()
    assert retired, "no edge was invalidated by a delivery that changed a content_hash"
    for rec_a, rec_b, invalidated_run in retired:
        assert SUPERSEDED in (str(rec_a), str(rec_b)), (
            f"({rec_a}, {rec_b}) was retired but is not incident to the superseded record"
        )
        assert invalidated_run is not None, "a retired row carries no invalidated_run_id (S5)"
    duplicates = sup.connection.execute(
        f"SELECT count(*) FROM {MATCH_SCORES} "
        "GROUP BY rec_a_key, rec_b_key, model_version, tf_snapshot_id HAVING count(*) > 1"
    ).fetchall()
    assert not duplicates, "invalidation left a second row for a logical key (S5.0)"

    # AC6: the record left its entity, and exactly one member_removed says so.
    after = dict(membership_pairs(sup.connection))
    assert after[SUPERSEDED] != prior_entity, (
        "the superseded record kept its entity; its new values match nobody"
    )
    for member in CO_MEMBERS:
        assert after[member] == prior_entity, (
            f"{member} moved, but nothing about it changed — only {SUPERSEDED} was re-delivered"
        )
    removed = int(
        scalar(
            sup.connection,
            f"SELECT count(*) FROM {EVENTS} WHERE run_id = ? AND event_type = ? AND entity_id = ?",
            sup.run_id,
            MEMBER_REMOVED,
            prior_entity,
        )
    )
    assert removed == 1, (
        f"{removed} member_removed events on the prior entity; S4.5.3 emits exactly one"
    )

    if os.environ.get(REGEN_ENV):
        _regenerate(sup, BATCH_PHASE)
        pytest.fail(f"{REGEN_ENV} was set: wrote {BATCH_PHASE} expectations to {ARTIFACTS_DIR}")

    expected = SCENARIO_DIR / "expected" / BATCH_PHASE / "membership.csv"
    assert expected.is_file(), (
        f"{expected} is not committed; generate it with {REGEN_ENV}=1 rather than by hand"
    )
    assert_partition_equal(membership_pairs(sup.connection), expected)
    assert_membership_equals_components(sup.connection)


def test_affected_set_widens_to_full_entity_membership(sup: Sup) -> None:
    """AC5: the reconcile saw co-members that appear in no batch delivery.

    The batch delivers one record. If S4.5.1's widening did not happen the reconcile
    would never have looked at the other two, and the entity they shared would still be
    holding a member that had gone. Asserted through the outcome — the co-members are
    still together and the departed one is not — because the affected set is internal to
    the stage and a test that reached into it would be testing the implementation.
    """
    before = dict(membership_pairs(sup.connection))
    prior_entity = before[SUPERSEDED]

    sup.phase(BATCH_PHASE)
    after = dict(membership_pairs(sup.connection))

    assert after[CO_MEMBERS[0]] == after[CO_MEMBERS[1]] == prior_entity, (
        "the co-members did not stay in the entity they shared; the widening either did "
        f"not happen or moved records nothing touched. before={before} after={after}"
    )
    assert after[SUPERSEDED] not in {prior_entity}, "the superseded record did not leave"
    assert_membership_equals_components(sup.connection)
