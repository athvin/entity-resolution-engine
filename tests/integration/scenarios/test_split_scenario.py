"""T-PERM-2: a `never` on a bridge severs an entity and rank 1 keeps the id (S4.5.3).

S8.3 lists T-PERM-2 at `tests/integration/test_permanence.py::
test_split_retains_id_for_rank_1_fragment`; the board realises it here with the same
function name, per ER-077's node-id note. The scenario is S8.2's `split_scenario`:
the base phase co-clusters two real people through one bridge edge, and the batch
phase asserts `never` over exactly that pair. Three properties only a real split can
show are what the module asserts:

* **The assertion delta alone reaches the entity.** The batch delivery contains
  neither asserted record (machine-checked by the fixture lint), so the affected
  node set includes them exclusively through S4.5.1's assertion-delta arm — the
  clause that makes `never` "applied identically in incremental and full modes"
  (S4.4) true for records no batch will ever redeliver.
* **No partition-level cut fires.** The asserted pair is a bridge, so the S4.4
  pre-clustering edge adjustment removes the component's only cross connection and
  S4.4.2 has nothing left to do: zero `cut_edges` rows, zero `edge_cut` events.
  This is exactly what distinguishes the scenario from `assertions_scenario`'s A3
  case, where a third record keeps the endpoints connected and a cut is the only
  way out.
* **Fragment ordering is a total order.** `(member_count DESC, min member
  record_key ASC)`: the 3-member fragment outranks the 2-member one and keeps the
  `entity_id`; `split_scenario_tie_2_2`'s even fragments fall through to the
  record-key element.

The expected files were generated from real runs under `ER_REGEN_SPLIT=1` and are
compared, never authored to match (S8.2.1). Regeneration rewrites them into the
mounted artifacts directory and then fails deliberately, so it can never leave a
green suite.
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

SPLIT_SCENARIO: Final = "split_scenario"
TIE_SCENARIO: Final = "split_scenario_tie_2_2"

BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
CUT_EDGES: Final = f"{SCHEMA_QUALIFIER}.cut_edges"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

SPLIT_EVENT: Final = "split"
EDGE_CUT_EVENT: Final = "edge_cut"

#: The fragments each severing leaves, and the batch newcomer. Spelled here so a test
#: reads as a claim about the fixture rather than as a query whose answer happens to
#: be five rows. The fixture lint derives the same structure from the CSVs.
SPLIT_RETAINED: Final[tuple[str, ...]] = ("billing:B301", "crm:C301", "webforms:W301")
SPLIT_MINTED: Final[tuple[str, ...]] = ("billing:B302", "crm:C302")
SPLIT_NEWCOMER: Final = "webforms:W303"

TIE_RETAINED: Final[tuple[str, ...]] = ("billing:B311", "crm:C311")
TIE_MINTED: Final[tuple[str, ...]] = ("billing:B312", "crm:C312")
TIE_NEWCOMER: Final = "webforms:W313"

#: A ULID: 26 characters of Crockford base32. The minted fragment's id must be one,
#: and must be a NEW one — novelty is asserted against the base id set.
ULID_SHAPE: Final = re.compile(r"^[0-9A-HJKMNP-TV-Z]{26}$")

#: Set to rewrite the committed expectations from a real run; the test then fails.
REGEN_ENV: Final = "ER_REGEN_SPLIT"


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
class Split:
    """One scenario driven through its phases, holding the base label map."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    scenario: Scenario
    dbt: Dbt
    root: Path
    name: str
    run_id: str = ""
    base_labels: dict[str, str] = field(default_factory=dict)
    base_entity_ids: frozenset[str] = frozenset()

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
        scored = run_er("match", "--mode", MODE_FULL, "--run-id", self.run_id, "--json")
        assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr
        reconciled = run_er("reconcile", "--run-id", self.run_id)
        assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr

    def apply_assertions(self, phase: str) -> None:
        """`er assert add` every scenario assertion of ``phase``, before the phase runs.

        Through the console script, not an INSERT: S8.2.1 makes `assertions.csv` an
        input the run loads, and the canonicalisation, conflict rejection and audit
        stamps all live in the S4.4 write path a test must not bypass.
        """
        for row in self.scenario.assertions.get(phase, ()):
            added = run_er(
                "assert",
                "add",
                "--a",
                str(row["rec_a_key"]),
                "--b",
                str(row["rec_b_key"]),
                "--kind",
                str(row["kind"]),
                "--by",
                str(row["created_by"]),
                "--note",
                str(row["note"]),
            )
            assert added.returncode == int(ExitCode.SUCCESS), added.stdout + added.stderr

    def batch(self) -> None:
        """The batch phase, with its assertions applied first (S8.2.1's phase column)."""
        self.apply_assertions(BATCH_PHASE)
        self.phase(BATCH_PHASE)

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


def _build(
    name: str,
    dbt_packages: None,
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Split]:
    """Run ``name``'s base phase and yield it, ready for `batch`."""
    scenario = load_scenario(name)
    assert scenario.phases == (BASE_PHASE, BATCH_PHASE)
    dbt = Dbt(connection=connection, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")

    database = str(scalar(connection, "SELECT current_database()"))
    schema = str(scalar(connection, "SELECT current_schema()"))
    try:
        # `load_fixture_model` points `params_path` at the committed file on disk,
        # which only an in-process scorer can read; `er match` fetches it through the
        # object store like any other run, so it is published and the row repointed.
        model_version, _, settings = load_fixture_model(connection)
        published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
        object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
        connection.execute(
            f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
            [published, model_version],
        )
        split = Split(
            connection=connection,
            cfg=cfg,
            scenario=scenario,
            dbt=dbt,
            root=tmp_path / "drop",
            name=name,
        )
        split.phase(BASE_PHASE)
        pairs = membership_pairs(connection)
        split.base_labels = label_map(pairs)
        split.base_entity_ids = frozenset(entity for _, entity in pairs)
        if os.environ.get(REGEN_ENV):
            _regenerate(split, BASE_PHASE)
        yield split
    finally:
        connection.execute(f'USE "{database}".{schema}')


@pytest.fixture
def split(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Split]:
    """`split_scenario` with its base phase applied."""
    yield from _build(SPLIT_SCENARIO, dbt_packages, initialised_lake, cfg, object_store, tmp_path)


@pytest.fixture
def tie(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Split]:
    """`split_scenario_tie_2_2` with its base phase applied."""
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


def _labels_after(split: Split) -> dict[str, str]:
    """The base map, extended with labels for entities minted since (S8.2.1 order)."""
    pairs = membership_pairs(split.connection)
    groups: dict[str, list[str]] = {}
    for record, entity in pairs:
        groups.setdefault(entity, []).append(record)
    by_id = {entity: label for label, entity in split.base_labels.items()}
    novel = sorted(
        (entity for entity in groups if entity not in by_id),
        key=lambda entity: min(groups[entity]),
    )
    ordinal = len(by_id)
    labels = dict(split.base_labels)
    for entity in novel:
        ordinal += 1
        labels[f"E{ordinal}"] = entity
    return labels


def _regenerate(split: Split, phase: str) -> None:
    """Write this phase's expected files into the mounted artifacts directory.

    Labels are the BASE phase's map extended in minted order, so a batch expectation
    names the retained fragment by the label it carried before the split — which is
    what makes the committed file show a split rather than a relabelling.
    """
    out = ARTIFACTS_DIR / split.name / phase
    out.mkdir(parents=True, exist_ok=True)

    pairs = membership_pairs(split.connection)
    labels = _labels_after(split) if phase == BATCH_PHASE else label_map(pairs)
    by_id = {entity: label for label, entity in labels.items()}
    personas = dict(_persona_ids(split.scenario, BASE_PHASE))
    personas.update(_persona_ids(split.scenario, phase))

    rows = sorted(
        (personas.get(record, "\\N"), *record.split(":", 1), by_id.get(entity, entity))
        for record, entity in pairs
    )
    (out / "membership.csv").write_text(
        "persona_id,source_system,source_record_id,entity_label\n"
        + "".join(f"{p},{s},{r},{label}\n" for p, s, r, label in rows),
        encoding="utf-8",
    )

    counted = split.events_of_run()
    (out / "events.csv").write_text(
        "entity_label,event_type,count\n"
        + "".join(
            f"{by_id.get(entity, entity)},{event_type},{count}\n"
            for entity, event_type, count in sorted(
                (by_id.get(entity, entity), event_type, count)
                for entity, event_type, count in counted
            )
        ),
        encoding="utf-8",
    )


def _assert_events_match_expectation(split: Split) -> None:
    """AC5's file half: `expected/batch/events.csv` is this run's events, resolved."""
    labels = _labels_after(split)
    by_id = {entity: label for label, entity in labels.items()}
    observed = sorted(
        (by_id.get(entity, entity), event_type, count)
        for entity, event_type, count in split.events_of_run()
    )
    committed_path = split.expected_dir / BATCH_PHASE / "events.csv"
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


def _cut_counts(split: Split) -> tuple[int, int]:
    """`(cut_edges rows, edge_cut events)`, both of which AC6 requires to be zero."""
    cut_rows = int(scalar(split.connection, f"SELECT count(*) FROM {CUT_EDGES}"))
    cut_events = int(
        scalar(
            split.connection,
            f"SELECT count(*) FROM {EVENTS} WHERE event_type = ?",
            EDGE_CUT_EVENT,
        )
    )
    return cut_rows, cut_events


# --------------------------------------------------------------------------- #
# T-PERM-2
# --------------------------------------------------------------------------- #


def test_split_retains_id_for_rank_1_fragment(split: Split) -> None:
    """AC4, AC8: the 3-member fragment keeps the entity_id; the 2-member one cannot."""
    base = dict(membership_pairs(split.connection))
    assert set(base) == {*SPLIT_RETAINED, *SPLIT_MINTED}, f"base delivered {sorted(base)}"
    assert len(set(base.values())) == 1, (
        f"the base phase did not co-cluster the bridge component into one entity: {base}. "
        "The bridge pair agrees on phone_e164, birth_date and addr_postal, which the "
        "committed model scores above auto_merge; if it did not, the fixture's premise "
        "is gone and there is nothing to sever."
    )
    entity_before = base[SPLIT_RETAINED[0]]

    if os.environ.get(REGEN_ENV):
        split.batch()
        _regenerate(split, BATCH_PHASE)
        pytest.fail(f"{REGEN_ENV} set: expectations rewritten under artifacts/, not asserted")

    assert_partition_equal(
        membership_pairs(split.connection), split.expected_dir / BASE_PHASE / "membership.csv"
    )
    assert_membership_equals_components(split.connection)

    split.batch()

    after = dict(membership_pairs(split.connection))
    assert set(after) == {*SPLIT_RETAINED, *SPLIT_MINTED, SPLIT_NEWCOMER}

    retained = {after[key] for key in SPLIT_RETAINED}
    assert retained == {entity_before}, (
        f"the rank-1 fragment's ids are {retained}, not the base entity "
        f"{entity_before}; S4.5.3 ranks fragments (member_count DESC, min record_key "
        "ASC) and the 3-member fragment outranks the 2-member one"
    )

    # AC4's second half: the minority fragment's id is newly minted — it appears
    # nowhere in the base-phase membership.
    minted = {after[key] for key in SPLIT_MINTED}
    assert len(minted) == 1
    assert not minted & split.base_entity_ids, (
        f"the minority fragment reused base-phase id(s) {minted & split.base_entity_ids}"
    )

    assert_ids_stable(
        membership_pairs(split.connection),
        split.expected_dir / BATCH_PHASE / "membership.csv",
        _labels_after(split),
    )
    assert_membership_equals_components(split.connection)


def test_minority_fragment_gets_new_ulid_and_one_split_event(split: Split) -> None:
    """AC5: exactly one `split` event for the batch run, encoded in events.csv."""
    split.batch()

    minted = {split.entity_of(key) for key in SPLIT_MINTED}
    assert len(minted) == 1
    minted_id = minted.pop()
    assert ULID_SHAPE.match(minted_id), f"{minted_id!r} is not a ULID (D10)"
    assert minted_id not in split.base_entity_ids

    split_events = [
        (entity, count)
        for entity, event_type, count in split.events_of_run()
        if event_type == SPLIT_EVENT
    ]
    assert split_events == [(minted_id, 1)], (
        f"expected exactly one split event, on the minted fragment {minted_id}, got {split_events}"
    )
    _assert_events_match_expectation(split)


def test_two_two_split_resolved_by_min_record_key(tie: Split) -> None:
    """AC7: even fragments, so `min member record_key ASC` is the only decider."""
    base = dict(membership_pairs(tie.connection))
    assert set(base) == {*TIE_RETAINED, *TIE_MINTED}, f"base delivered {sorted(base)}"
    assert len(set(base.values())) == 1, f"the tie base is not one 4-member entity: {base}"
    assert len(TIE_RETAINED) == len(TIE_MINTED), (
        "the two fragments differ in size, so member_count decides and the tiebreak "
        "is never reached — the scenario would not test what it exists to test"
    )
    assert min(TIE_RETAINED) < min(TIE_MINTED), (
        f"{min(TIE_RETAINED)!r} does not sort before {min(TIE_MINTED)!r}; the "
        "fixture's own premise about which fragment retains the id is wrong"
    )
    entity_before = base[TIE_RETAINED[0]]

    if os.environ.get(REGEN_ENV):
        tie.batch()
        _regenerate(tie, BATCH_PHASE)
        pytest.fail(f"{REGEN_ENV} set: expectations rewritten under artifacts/, not asserted")

    tie.batch()

    after = dict(membership_pairs(tie.connection))
    assert {after[key] for key in TIE_RETAINED} == {entity_before}, (
        "the fragment holding the smaller minimum record_key did not keep the id; "
        "reversing the committed expectation must fail this test (AC7), so the "
        "assertion is against the live ids, not only the file"
    )
    minted = {after[key] for key in TIE_MINTED}
    assert len(minted) == 1 and not minted & tie.base_entity_ids

    split_events = [
        (entity, count)
        for entity, event_type, count in tie.events_of_run()
        if event_type == SPLIT_EVENT
    ]
    assert split_events == [(minted.pop(), 1)]

    assert_ids_stable(
        membership_pairs(tie.connection),
        tie.expected_dir / BATCH_PHASE / "membership.csv",
        _labels_after(tie),
    )
    _assert_events_match_expectation(tie)
    assert_membership_equals_components(tie.connection)

    cut_rows, cut_events = _cut_counts(tie)
    assert (cut_rows, cut_events) == (0, 0), (
        f"the tie scenario cut ({cut_rows} rows, {cut_events} events); severing a "
        "bridge needs no partition-level cut (AC6)"
    )


def test_no_cut_edges_written(split: Split) -> None:
    """AC6: the S4.4 edge adjustment removes the bridge; S4.4.2 never fires."""
    split.batch()
    cut_rows, cut_events = _cut_counts(split)
    assert (cut_rows, cut_events) == (0, 0), (
        f"severing the bridge produced {cut_rows} cut_edges row(s) and {cut_events} "
        "edge_cut event(s); the asserted pair is the component's only connection, so "
        "the pre-clustering adjustment alone must separate it (S4.4, S4.4.2) — a cut "
        "here means the fixture has a second cross edge or the adjustment did not run"
    )
    assert_membership_equals_components(split.connection)
