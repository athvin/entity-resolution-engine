"""B5: the two clustering paths agree, and the threshold is the cut (S4.5.2, S4.3).

S4.5.2 specifies clustering twice — incrementally as min-label propagation over the
affected subgraph, and for a full re-resolution as Splink's
`cluster_pairwise_predictions_at_threshold` — and B5 requires that **both paths consume
the identical edge set** and produce the identical partition. Nothing in the scoring
layer can catch a disagreement: every individual `match_probability` would be equal on
both sides. The partition is the only place the drift is visible, which is why this
module compares partitions and why T-INV-1 re-checks the same property after every
scenario.

The comparison is bidirectional set equality of frozensets, never a count. Two
partitions of 23 records into 10 components can differ in which records sit where, and
a count comparison calls that agreement.

**Why a sub-threshold edge gets its own test.** Splink's clustering takes
`threshold_match_probability=None` by default and a `None` threshold treats every
supplied edge as a match, so `cluster_full` passing the argument is load-bearing.
`tests/unit/entities/test_cluster_threshold.py` spies on the call; this module asserts
the observable consequence — an edge below `auto_merge` leaves its endpoints in
different components — because a spy proves the argument was passed and only the
partition proves it was honoured.

The `Dbt` harness and the delivery helper are duplicated from
`tests/integration/test_full_match.py` rather than imported, for the reason that module
states: a test module importing another test module makes a node id's dependencies
invisible to whoever reads it.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.invariants import assert_membership_equals_components, membership_partition
from helpers.model import fixture_settings, load_fixture_model
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.cluster import Edge, cluster_full, label_propagate
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.api import assert_no_splink_relations_in_lake
from er.matching.full import score_full
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.obs.counters import DECLARED_COUNTERS, StageCounters
from er.obs.runctx import StageRun

SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
ENTITY_MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
ENTITIES: Final = f"{SCHEMA_QUALIFIER}.entities"

MATCH_STAGE: Final = "match"

#: A partition, as both paths return it.
Partition = frozenset[frozenset[str]]


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


def deliver(scenario: Scenario, root: Path) -> Path:
    """Materialise the scenario's phase as the drop-folder root `er ingest --path` reads."""
    for source, path in scenario.inputs_for(PHASE).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def partition_of(labels: dict[str, str]) -> Partition:
    """A `record_key -> label` mapping as a set-partition."""
    grouped: dict[str, set[str]] = {}
    for key, label in labels.items():
        grouped.setdefault(label, set()).add(key)
    return frozenset(frozenset(members) for members in grouped.values())


def difference_report(propagated: Partition, clustered: Partition) -> str:
    """Both directions of the symmetric difference, which is what AC1 requires."""
    only_propagated = sorted(sorted(group) for group in propagated - clustered)
    only_clustered = sorted(sorted(group) for group in clustered - propagated)
    return "\n".join(
        [
            "clustering parity (B5, S4.5.2): label propagation and Splink connected "
            f"components disagree — {len(propagated)} vs {len(clustered)} component(s)",
            f"  only from label_propagate ({len(only_propagated)}):",
            *(f"    {group}" for group in only_propagated[:20]),
            f"  only from cluster_full ({len(only_clustered)}):",
            *(f"    {group}" for group in only_clustered[:20]),
        ]
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
        """The selection `er standardize` runs: staging, then intermediate (S4.2)."""
        self("build", select=STAGING_SELECTOR)
        self("build", select=INTERMEDIATE_SELECTOR)

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


@dataclass(frozen=True)
class Subgraph:
    """`base_10` scored: the node set and the edge set both paths consume."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    nodes: tuple[str, ...]
    edges: tuple[Edge, ...]

    @property
    def pairs(self) -> tuple[tuple[str, str], ...]:
        """The edge set as canonical pairs, which is what `label_propagate` takes."""
        return tuple(edge.pair for edge in self.edges)

    def above_threshold(self) -> tuple[tuple[str, str], ...]:
        """The pairs at or above `auto_merge` — the cut label propagation must be given.

        `cluster_full` is handed the whole edge set and applies the cut itself, which is
        the argument under test. Label propagation has no threshold, so the caller
        applies it. Giving both paths the same *post-cut* edge set would make the
        comparison blind to the very argument this module exists to check.
        """
        return tuple(
            edge.pair
            for edge in self.edges
            if edge.match_probability >= self.cfg.thresholds.auto_merge
        )


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return config()


@pytest.fixture
def base_10() -> Scenario:
    """The S8.2 fixture both paths cluster."""
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (PHASE,)
    return scenario


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
def subgraph(
    base_10: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[Subgraph]:
    """`base_10` ingested, standardized and scored — the real edge set, not a synthetic one.

    The teardown restores the connection's default schema: `splink_api` repoints the
    shared session handle at the scratch schema.
    """
    delivery = deliver(base_10, tmp_path / "drop")
    for source in base_10.inputs_for(PHASE):
        result = run_er("ingest", "--source", source, "--path", str(delivery))
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")
    dbt.standardize()

    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        model_version, tf_snapshot_id, _ = load_fixture_model(initialised_lake)
        score_full(
            initialised_lake,
            cfg,
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
        nodes = tuple(
            str(row[0])
            for row in initialised_lake.execute(
                f"SELECT record_key FROM {STD_RECORDS} ORDER BY record_key"
            ).fetchall()
        )
        edges = tuple(
            Edge(rec_a_key=str(left), rec_b_key=str(right), match_probability=float(probability))
            for left, right, probability in initialised_lake.execute(
                f"SELECT rec_a_key, rec_b_key, match_probability FROM {MATCH_SCORES} "
                "WHERE is_active ORDER BY rec_a_key, rec_b_key"
            ).fetchall()
        )
        assert nodes and edges, "base_10 scored nothing; every comparison below is vacuous"
        yield Subgraph(connection=initialised_lake, cfg=cfg, nodes=nodes, edges=edges)
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def test_label_propagation_equals_splink_components(subgraph: Subgraph) -> None:
    """AC1: both S4.5.2 implementations induce the same partition over one edge set."""
    propagated = partition_of(
        label_propagate(
            subgraph.connection,
            subgraph.nodes,
            subgraph.above_threshold(),
            max_iterations=subgraph.cfg.clustering.max_iterations,
        ).labels
    )
    clustered = cluster_full(
        subgraph.connection,
        subgraph.nodes,
        subgraph.edges,
        auto_merge=subgraph.cfg.thresholds.auto_merge,
        settings=fixture_settings(),
    )

    assert propagated, "label propagation returned no components"
    assert clustered, "Splink clustering returned no components"
    assert propagated == clustered, difference_report(propagated, clustered)
    # Every record is placed exactly once by both, which is what makes them partitions
    # rather than coverings — a component that appeared in both but overlapped another
    # would satisfy set equality and still be wrong.
    assert sorted(key for group in propagated for key in group) == sorted(subgraph.nodes)

    assert_no_splink_relations_in_lake(subgraph.connection)


def test_sub_threshold_edge_is_not_clustered(subgraph: Subgraph) -> None:
    """AC3: an edge below `auto_merge` leaves its endpoints in different components.

    The observable consequence of passing `threshold_match_probability`. A crafted edge
    is used rather than a naturally occurring one because `base_10`'s duplicates score
    near 1.0 and the natural band may hold nothing — the same reason
    `test_full_match.py` crafts a threshold for its gray-band arm.
    """
    auto_merge = subgraph.cfg.thresholds.auto_merge
    left, right = sorted(subgraph.nodes)[:2]
    crafted = (Edge(rec_a_key=left, rec_b_key=right, match_probability=auto_merge - 0.2),)
    clustered = cluster_full(
        subgraph.connection,
        [left, right],
        crafted,
        auto_merge=auto_merge,
        settings=fixture_settings(),
    )

    assert clustered == frozenset({frozenset({left}), frozenset({right})}), (
        f"an edge at {auto_merge - 0.2} was clustered at a cut of {auto_merge}; either "
        "threshold_match_probability was not passed, or it was not honoured. "
        f"Components: {sorted(sorted(group) for group in clustered)}"
    )

    # And the converse, so the assertion above cannot pass because clustering is inert.
    merged = cluster_full(
        subgraph.connection,
        [left, right],
        (Edge(rec_a_key=left, rec_b_key=right, match_probability=auto_merge),),
        auto_merge=auto_merge,
        settings=fixture_settings(),
    )
    assert merged == frozenset({frozenset({left, right})}), (
        f"an edge at exactly {auto_merge} was NOT clustered; the band is half-open and "
        "p == auto_merge is a match (S4.3)"
    )


def test_invariant_helper_detects_membership_drift(subgraph: Subgraph) -> None:
    """AC4/AC5: T-INV-1 catches a repointed membership row, and passes once restored.

    `entity_membership` is empty until ER-074, so the rows are written here. That is the
    only way to exercise clause 1 at this point on the board, and exercising it now is
    what stops the clause being dead code for the whole of M4.
    """
    # Clause 4 and 5 hold on the scored lake before anything is inserted (AC5).
    assert_membership_equals_components(subgraph.connection)
    assert membership_partition(subgraph.connection) == frozenset(), (
        "entity_membership is not empty; this test writes the rows it asserts on and "
        "would otherwise be interpreting somebody else's"
    )

    labels = label_propagate(
        subgraph.connection,
        subgraph.nodes,
        subgraph.above_threshold(),
        max_iterations=subgraph.cfg.clustering.max_iterations,
    ).labels
    _write_membership(subgraph.connection, labels)

    # Truthful membership: the helper agrees with the recomputation.
    assert_membership_equals_components(
        subgraph.connection,
        nodes=list(subgraph.nodes),
        edges=list(subgraph.above_threshold()),
        max_iterations=subgraph.cfg.clustering.max_iterations,
    )

    # Repoint exactly one record at another component's entity, and it must be caught.
    victim, elsewhere = _two_distinct_entities(labels)
    subgraph.connection.execute(
        f"UPDATE {ENTITY_MEMBERSHIP} SET entity_id = ? WHERE record_key = ?",
        [elsewhere, victim],
    )
    with pytest.raises(AssertionError) as drift:
        assert_membership_equals_components(
            subgraph.connection,
            nodes=list(subgraph.nodes),
            edges=list(subgraph.above_threshold()),
            max_iterations=subgraph.cfg.clustering.max_iterations,
        )
    assert victim in str(drift.value), (
        f"the failure does not name the repointed record {victim!r}: {drift.value}"
    )

    # Restored, it passes again — so the assertion above failed for the drift and not
    # for something this test did on its way there.
    subgraph.connection.execute(
        f"UPDATE {ENTITY_MEMBERSHIP} SET entity_id = ? WHERE record_key = ?",
        [labels[victim], victim],
    )
    assert_membership_equals_components(
        subgraph.connection,
        nodes=list(subgraph.nodes),
        edges=list(subgraph.above_threshold()),
        max_iterations=subgraph.cfg.clustering.max_iterations,
    )

    # Leave the namespace as it was found: the autouse finalizer runs T-INV-1 again on
    # the way out, and it would otherwise read rows this test invented.
    subgraph.connection.execute(f"DELETE FROM {ENTITY_MEMBERSHIP}")
    subgraph.connection.execute(f"DELETE FROM {ENTITIES}")


def _write_membership(connection: duckdb.DuckDBPyConnection, labels: dict[str, str]) -> None:
    """Write one `entities` row per component and one `entity_membership` row per record."""
    stamp = datetime.now(UTC)
    for entity_id in sorted(set(labels.values())):
        connection.execute(
            f"INSERT INTO {ENTITIES} (entity_id, status, merged_into, created_at, "
            "updated_at, created_run_id, updated_run_id) VALUES (?, 'active', NULL, ?, ?, ?, ?)",
            [entity_id, stamp, stamp, str(ULID()), str(ULID())],
        )
    for record, entity_id in sorted(labels.items()):
        source_system, source_record_id = record.split(":", 1)
        connection.execute(
            f"INSERT INTO {ENTITY_MEMBERSHIP} (source_system, source_record_id, record_key, "
            "entity_id, assigned_at, run_id) VALUES (?, ?, ?, ?, ?, ?)",
            [source_system, source_record_id, record, entity_id, stamp, str(ULID())],
        )


def _two_distinct_entities(labels: dict[str, str]) -> tuple[str, str]:
    """A record, and the label of a component it does not belong to."""
    distinct: Sequence[str] = sorted(set(labels.values()))
    assert len(distinct) >= 2, (
        f"base_10 clustered into {len(distinct)} component(s); the drift arm needs two "
        "so a record can be repointed at another one"
    )
    victim = next(record for record, label in sorted(labels.items()) if label == distinct[0])
    return victim, distinct[1]
