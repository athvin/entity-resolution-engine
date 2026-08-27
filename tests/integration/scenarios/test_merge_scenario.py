"""T-PERM-1: a bridging record merges two entities and the survivor keeps its id (S4.5.3).

S4.5.3 states the reconciliation as one overlap matrix rather than a branch per shape,
and a merge is the case where an old entity loses **all** of its members to another's
claim. Two properties of that rule are only observable with a real merge in front of
them, and nothing else in the suite constructs one:

* **The claimant is the largest overlap, and `min member record_key` is only a
  tiebreak.** `merge_scenario` gives the SMALLER old entity the SMALLER minimum record
  key on purpose. An implementation that consulted the tiebreak first would pick the
  two-member entity and every assertion below would still be satisfiable by swapping the
  committed expectation — which is exactly why the expectation is generated from a run
  and then compared, never authored to match.
* **The loser is redirected, not deleted.** `entities.merged_into` exists only for
  external id resolution (S4.5.3): `ids.resolve()` must still answer for an id a caller
  wrote down before the merge, while `entity_membership` — current state — must hold
  zero rows referencing it.

**Why a merge is constructible here when a bridge was not.** ER-077 measured that
`model_test_v1` admits no intermediate-strength link and concluded every component is a
clique, which would make this fixture impossible too. That holds for records whose
compared attributes are all present. It does not hold across a NULL: every comparison in
the committed model carries `is_null_level`, and Splink scores a null level at a Bayes
factor of 1 — no evidence either way — where an actual disagreement on `phone_e164`
contributes ~1.6e-18 and annihilates the score. So a record delivered with no phone and
no email matches members of both old entities (~0.999) while those entities still score
~1e-16 against each other. The asymmetry is the fixture.

The expected files are generated from a real run under `ER_REGEN_MERGE=1`, which
rewrites them into the mounted artifacts directory and then fails deliberately. Which
records co-cluster is a property of the committed model, so it is measured rather than
declared (S8.2.1).
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
from er.entities.ids import resolve
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.full import MODE_FULL
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
FIXTURE_ROOT: Final = REPO_ROOT / "fixtures" / "static"

MERGE_SCENARIO: Final = "merge_scenario"
TIE_SCENARIO: Final = "merge_scenario_tie"

BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
ENTITIES: Final = f"{SCHEMA_QUALIFIER}.entities"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

MERGED: Final = "merged"

#: The records each scenario's base phase places in two entities, and the bridge that
#: joins them. Spelled here so a test reads as a claim about the fixture rather than as
#: a query whose answer happens to be three rows.
MERGE_SMALL: Final[tuple[str, ...]] = ("billing:B501", "crm:C501")
MERGE_LARGE: Final[tuple[str, ...]] = ("billing:B510", "crm:C510", "webforms:W510")
MERGE_BRIDGE: Final = "webforms:W503"

TIE_FIRST: Final[tuple[str, ...]] = ("billing:B601", "crm:C601")
TIE_SECOND: Final[tuple[str, ...]] = ("billing:B602", "webforms:W602")
TIE_BRIDGE: Final = "webforms:W603"

#: Set to rewrite the committed expectations from a real run; the test then fails.
REGEN_ENV: Final = "ER_REGEN_MERGE"


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
    """`entity_label -> entity_id`, allocated by ascending minimum member record_key.

    The DoD's allocation rule, and the map `assert_ids_stable` resolves a committed
    `entity_label` through. It is captured after the BASE phase and reused for the batch
    comparison: rebuilding it after the merge would relabel the survivor to whatever it
    then happened to be, and the stability assertion would pass vacuously.
    """
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
class Merge:
    """One scenario driven through its phases, holding the base label map."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    scenario: Scenario
    dbt: Dbt
    root: Path
    name: str
    run_id: str = ""
    base_labels: dict[str, str] = field(default_factory=dict)

    @property
    def expected_dir(self) -> Path:
        return FIXTURE_ROOT / self.name / "expected"

    def phase(self, name: str) -> None:
        """Deliver, ingest, standardize, score and reconcile one phase under one run."""
        self.run_id = str(ULID())
        delivery = deliver(self.scenario, name, self.root / name)
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
        self.dbt.standardize()

        # `er match` rather than an in-process `score_full`: S4.5.5 invalidation is a
        # property of the stage, not the scorer (S4.0b permits the scoring path one
        # write), so the stage is what a scenario must drive.
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
    name: str,
    dbt_packages: None,
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Merge]:
    """Run ``name``'s base phase and yield it, ready for `batch`."""
    scenario = load_scenario(name)
    assert scenario.phases == (BASE_PHASE, BATCH_PHASE)
    dbt = Dbt(connection=connection, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")

    database = str(scalar(connection, "SELECT current_database()"))
    schema = str(scalar(connection, "SELECT current_schema()"))
    try:
        # `load_fixture_model` points `params_path` at the committed file on disk, which
        # only an in-process scorer can read; `er match` fetches it through the object
        # store like any other run, so it is published and the row repointed.
        model_version, _, settings = load_fixture_model(connection)
        published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
        object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
        connection.execute(
            f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
            [published, model_version],
        )
        merge = Merge(
            connection=connection,
            cfg=cfg,
            scenario=scenario,
            dbt=dbt,
            root=tmp_path / "drop",
            name=name,
        )
        merge.phase(BASE_PHASE)
        merge.base_labels = label_map(membership_pairs(connection))
        if os.environ.get(REGEN_ENV):
            # Captured here because by the time a test has run the batch phase the
            # state this file describes no longer exists.
            _regenerate(merge, BASE_PHASE)
        yield merge
    finally:
        connection.execute(f'USE "{database}".{schema}')


@pytest.fixture
def merge(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Merge]:
    """`merge_scenario` with its base phase applied."""
    yield from _build(MERGE_SCENARIO, dbt_packages, initialised_lake, cfg, object_store, tmp_path)


@pytest.fixture
def tie(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Merge]:
    """`merge_scenario_tie` with its base phase applied."""
    yield from _build(TIE_SCENARIO, dbt_packages, initialised_lake, cfg, object_store, tmp_path)


def _persona_ids(scenario: Scenario, phase: str) -> dict[str, str]:
    """`record_key -> persona_id` from the delivered CSVs of ``phase`` (S8.2.1)."""
    labels: dict[str, str] = {}
    for source, path in scenario.inputs_for(phase).items():
        for line in path.read_text(encoding="utf-8").splitlines()[1:]:
            if line:
                fields = line.split(",")
                labels[f"{source}:{fields[0]}"] = fields[-1]
    return labels


def _regenerate(merge: Merge, phase: str) -> None:
    """Write this phase's expected files into the mounted artifacts directory.

    S7.1's image COPYs the repository rather than bind-mounting it, so a file written at
    the fixture path inside the container never reaches the host. Only `artifacts/` is
    mounted, so that is where a containerised regeneration is collected from.

    Labels are always the BASE phase's map, so a batch expectation names the survivor by
    the label it carried before the merge — which is what makes the committed file show
    a merge rather than a relabelling.
    """
    out = ARTIFACTS_DIR / merge.name / phase
    out.mkdir(parents=True, exist_ok=True)

    pairs = membership_pairs(merge.connection)
    labels = merge.base_labels or label_map(pairs)
    by_id = {entity: label for label, entity in labels.items()}
    personas = dict(_persona_ids(merge.scenario, BASE_PHASE))
    personas.update(_persona_ids(merge.scenario, phase))

    rows = sorted(
        (personas.get(record, "\\N"), *record.split(":", 1), by_id.get(entity, entity))
        for record, entity in pairs
    )
    (out / "membership.csv").write_text(
        "persona_id,source_system,source_record_id,entity_label\n"
        + "".join(f"{p},{s},{r},{label}\n" for p, s, r, label in rows),
        encoding="utf-8",
    )

    counted = merge.connection.execute(
        f"SELECT entity_id, event_type, count(*) FROM {EVENTS} WHERE run_id = ? "
        "GROUP BY entity_id, event_type ORDER BY entity_id, event_type",
        [merge.run_id],
    ).fetchall()
    (out / "events.csv").write_text(
        "entity_label,event_type,count\n"
        + "".join(
            f"{by_id.get(str(entity), str(entity))},{event_type},{count}\n"
            for entity, event_type, count in counted
        ),
        encoding="utf-8",
    )


def _redirects(merge: Merge) -> dict[str, str]:
    """`entity_id -> merged_into` for every entity that has one.

    :func:`~er.entities.ids.resolve` takes the redirect map rather than a connection, so
    that the cycle guard is testable without a lake (S4.5.3). This is the read that
    supplies it.
    """
    return {
        str(entity): str(target)
        for entity, target in merge.connection.execute(
            f"SELECT entity_id, merged_into FROM {ENTITIES} WHERE merged_into IS NOT NULL"
        ).fetchall()
    }


def _merged_events(merge: Merge) -> list[tuple[str, int]]:
    """`(entity_id, count)` per `merged` event of the current run."""
    return [
        (str(entity), int(count))
        for entity, count in merge.connection.execute(
            f"SELECT entity_id, count(*) FROM {EVENTS} WHERE run_id = ? AND event_type = ? "
            "GROUP BY entity_id ORDER BY entity_id",
            [merge.run_id, MERGED],
        ).fetchall()
    ]


def test_merge_preserves_survivor_id(merge: Merge) -> None:
    """T-PERM-1 / AC2, AC4, AC8: the survivor keeps its id across a real merge."""
    base = dict(membership_pairs(merge.connection))
    assert set(base) == {*MERGE_SMALL, *MERGE_LARGE}, f"base delivered {sorted(base)}"

    small = {base[key] for key in MERGE_SMALL}
    large = {base[key] for key in MERGE_LARGE}
    assert len(small) == 1 and len(large) == 1, (
        f"the base phase did not form two whole entities: {base}"
    )
    assert small != large, (
        "the base phase already merged the two groups, so there is nothing left to "
        f"merge: {base}. The two are meant to be kept apart by phone and email alone."
    )
    survivor_before = large.pop()
    loser_before = small.pop()

    # Under regen the committed expectations do not exist yet, so every comparison below
    # would fail on a missing file rather than on the thing it asserts. The structural
    # claims above still run first: a regeneration that captured a base partition which
    # was already one entity would write an expectation for a merge that never happened.
    if os.environ.get(REGEN_ENV):
        merge.phase(BATCH_PHASE)
        _regenerate(merge, BATCH_PHASE)
        pytest.fail(f"{REGEN_ENV} set: expectations rewritten under artifacts/, not asserted")

    assert_partition_equal(
        membership_pairs(merge.connection), merge.expected_dir / BASE_PHASE / "membership.csv"
    )
    assert_membership_equals_components(merge.connection)

    merge.phase(BATCH_PHASE)

    after = dict(membership_pairs(merge.connection))
    assert set(after) == {*MERGE_SMALL, *MERGE_LARGE, MERGE_BRIDGE}, (
        f"the batch phase did not place the bridge: {sorted(after)}"
    )
    assert len(set(after.values())) == 1, (
        f"the bridge did not merge the two entities: {after}. It carries no phone and "
        "no email so both comparisons hit is_null_level; if either standardized to a "
        "literal instead of NULL the merge cannot happen."
    )

    # AC4: the claimant is the LARGER overlap (3 members), not the smaller minimum
    # record key. `billing:B501` sorts before `billing:B510`, so a tiebreak-first
    # implementation would have chosen the other entity and this is where it shows.
    assert after[MERGE_BRIDGE] == survivor_before, (
        f"the survivor is {after[MERGE_BRIDGE]}, not the 3-member entity "
        f"{survivor_before}; S4.5.3 claims by largest overlap and only then by "
        "min member record_key"
    )
    assert_ids_stable(
        membership_pairs(merge.connection),
        merge.expected_dir / BATCH_PHASE / "membership.csv",
        merge.base_labels,
    )

    # AC2: exactly one `merged` event, and it is emitted on the LOSER. S4.5.3 words the
    # rule as "old entities that lose all their members to a claimant are merged into
    # it ... one `merged` event", so the event is about the entity that was merged away
    # rather than about the one that absorbed it — the survivor's own row for this run
    # is `member_added`, which is a different claim and is asserted through events.csv.
    assert _merged_events(merge) == [(loser_before, 1)], (
        f"expected exactly one merged event, on the loser {loser_before}, got "
        f"{_merged_events(merge)}"
    )
    assert_partition_equal(
        membership_pairs(merge.connection), merge.expected_dir / BATCH_PHASE / "membership.csv"
    )
    assert_membership_equals_components(merge.connection)


def test_loser_is_redirected_and_unreferenced(merge: Merge) -> None:
    """AC3, AC5: `merged_into` resolves the old id; membership references it zero times."""
    base = dict(membership_pairs(merge.connection))
    loser = base[MERGE_SMALL[0]]
    survivor = base[MERGE_LARGE[0]]

    merge.phase(BATCH_PHASE)

    status, merged_into = merge.connection.execute(
        f"SELECT status, merged_into FROM {ENTITIES} WHERE entity_id = ?", [loser]
    ).fetchall()[0]
    assert str(status) == MERGED, f"the loser's status is {status!r}, not {MERGED!r}"
    assert str(merged_into) == survivor, (
        f"merged_into is {merged_into!r}, not the survivor {survivor!r}"
    )

    # AC3: an id a caller wrote down before the merge still resolves (S4.5.3).
    assert resolve(loser, _redirects(merge)) == survivor, (
        "ids.resolve did not follow merged_into to the survivor"
    )

    # AC5: and current state holds no row referencing it.
    referencing = int(
        scalar(merge.connection, f"SELECT count(*) FROM {MEMBERSHIP} WHERE entity_id = ?", loser)
    )
    assert referencing == 0, (
        f"entity_membership still holds {referencing} row(s) for the merged-away entity; "
        "S4.5.3 rewrites them to the claimant in the same snapshot"
    )


def test_claimant_tiebreak_selects_min_record_key_survivor(tie: Merge) -> None:
    """AC6: equal overlap, so `min member record_key ASC` is the only decider."""
    base = dict(membership_pairs(tie.connection))
    assert set(base) == {*TIE_FIRST, *TIE_SECOND}, f"base delivered {sorted(base)}"

    first = {base[key] for key in TIE_FIRST}
    second = {base[key] for key in TIE_SECOND}
    assert len(first) == 1 and len(second) == 1 and first != second, (
        f"the tie scenario's base phase is not two entities of two: {base}"
    )
    assert len(TIE_FIRST) == len(TIE_SECOND), (
        "the two old entities differ in size, so the overlap decides and the tiebreak "
        "is never reached — the scenario would not test what it exists to test"
    )
    expected_survivor = first.pop()
    expected_loser = second.pop()

    assert min(TIE_FIRST) < min(TIE_SECOND), (
        f"{min(TIE_FIRST)!r} does not sort before {min(TIE_SECOND)!r}; the fixture's "
        "own premise about which entity should survive is wrong"
    )

    tie.phase(BATCH_PHASE)
    if os.environ.get(REGEN_ENV):
        _regenerate(tie, BATCH_PHASE)
        pytest.fail(f"{REGEN_ENV} set: expectations rewritten under artifacts/, not asserted")

    after = dict(membership_pairs(tie.connection))
    assert len(set(after.values())) == 1, f"the tie bridge did not merge the two: {after}"
    assert after[TIE_BRIDGE] == expected_survivor, (
        f"the survivor is {after[TIE_BRIDGE]}, not {expected_survivor} — the entity "
        f"holding {min(TIE_FIRST)}, which is the smaller minimum member record_key of "
        "two equal overlaps (S4.5.3)"
    )
    assert _merged_events(tie) == [(expected_loser, 1)], (
        f"expected one merged event, on the loser {expected_loser}, got {_merged_events(tie)}"
    )
    assert_partition_equal(
        membership_pairs(tie.connection), tie.expected_dir / BATCH_PHASE / "membership.csv"
    )
    assert_membership_equals_components(tie.connection)
