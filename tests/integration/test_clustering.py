"""S4.5.2's propagation over `base_10`'s affected subgraph, against a real lake.

The algorithm's claims — the closed neighbourhood, the `ceil(log2 n) + 1` bound, isolated
nodes, order independence, the non-convergence raise — are
`tests/unit/entities/test_label_propagation.py`'s, and none of them needs a lake. Two
claims do, and they are the only ones here:

* **The partition is the connected components of the affected edge set.** T-INV-1 asserts
  that `entity_membership` equals the connected components of the current edge set at the
  end of every scenario, and S4.5.2 calls that "the only guard against the two
  implementations drifting apart". This suite is that comparison one stage earlier: the
  labelling this loop produces, against a reference implementation written a different way
  in the test, over an edge set that came out of the real `affected_nodes` /
  `affected_edges` chain rather than out of a literal list.
* **The loop writes nothing.** S4.0b binds the iterations to the in-memory database, "so
  the loop cannot commit one snapshot per iteration", and M17 is the same rule for Splink.
  Both are properties of a lake and neither is expressible against a `duckdb.connect()`:
  what is asserted is that the relation set of `lake.main` is unchanged across the call,
  that the DuckLake snapshot did not move, and that
  :func:`~er.matching.api.assert_no_splink_relations_in_lake` still passes.

**The edges are inserted, not scored.** Nothing here runs Splink or a model: what is under
test is the partition over a KNOWN graph, and scoring `base_10` to obtain one would make
the components a property of the frozen model. The `(model_version, tf_snapshot_id)` pair
is a literal for the same reason, exactly as `tests/integration/test_affected_edges.py`
uses one.

**The chain's minimum is a `billing` record.** `record_key` sorts lexicographically
(S5.0), so `billing:B001` is below every `crm:` and `webforms:` key in its component — the
label of a component is its minimum key and not the first record the edge list mentions,
and a component built the other way round would let a left-to-right sweep look correct.

The `Dbt` harness and the delivery helpers are duplicated from
`tests/integration/test_affected_edges.py` rather than imported, for the reason that module
states: a test module importing another test module makes a node id's dependencies
invisible to whoever reads it.
"""

from __future__ import annotations

import os
import subprocess
from collections import deque
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.cluster import (
    LABEL_PROP_ITERATIONS,
    MAX_ITERATION_BOUND,
    AffectedSet,
    Edge,
    affected_edges,
    affected_nodes,
    label_propagate,
)
from er.entities.ids import record_key
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, current_snapshot, detach
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.matching.api import assert_no_splink_relations_in_lake
from er.matching.edges import current_edges
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

#: The S8.2 fixture this suite standardizes, and its only phase.
SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

#: The two dbt selectors `er standardize` runs (S4.2).
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"

#: The scoring key every `match_scores` row here carries, and the one the loader is asked
#: for. Literal because nothing in this suite scores.
MODEL_VERSION: Final = "v0001"
TF_SNAPSHOT_ID: Final = "01JWMDTFSNAP00000000000071"

#: `thresholds.auto_merge` for this suite, and a probability above it. The corpus is never
#: scored, so both are the test's to choose.
AUTO_MERGE: Final = 0.90
SCORED_PROBABILITY: Final = 0.99

SCORED_AT: Final = datetime(2026, 3, 1, 0, 0, 0)

#: The three components the inserted edges induce over `base_10`, as `record_key`s built
#: through S5.0's helper. A chain of four, a chain of three and a pair: the chains are what
#: make the propagation take more than one round, and every other record of the corpus is
#: isolated, which is the S4.5.3 singleton input.
P1_CHAIN: Final[tuple[str, ...]] = (
    record_key("billing", "B001"),
    record_key("crm", "C001"),
    record_key("webforms", "W001"),
    record_key("webforms", "W002"),
)
P2_CHAIN: Final[tuple[str, ...]] = (
    record_key("billing", "B002"),
    record_key("crm", "C002"),
    record_key("webforms", "W003"),
)
P8_PAIR: Final[tuple[str, ...]] = (
    record_key("webforms", "W007"),
    record_key("webforms", "W008"),
)

#: One record of the corpus that no inserted edge touches, so the singleton claim is about
#: a named row rather than about a count.
ISOLATED: Final = record_key("crm", "C006")


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


def ingest(source: str, path: Path, *args: str) -> subprocess.CompletedProcess[str]:
    """One `er ingest` invocation, asserted to have succeeded."""
    result = run_er("ingest", "--source", source, "--path", str(path), *args)
    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
    return result


def deliver(rows: Mapping[str, str], root: Path, source: str) -> Path:
    """Write one source's CSV text under the drop-folder layout `er ingest` reads."""
    directory = root / source
    directory.mkdir(parents=True, exist_ok=True)
    for name, text in rows.items():
        (directory / name).write_text(text, encoding="utf-8")
    return root


def insert_row(
    connection: duckdb.DuckDBPyConnection, relation: str, values: Mapping[str, Any]
) -> None:
    """One row of a `ddl.py`-owned relation, projected through S5's own column list."""
    columns = REGISTRY[relation].column_names
    unknown = sorted(set(values) - set(columns))
    assert not unknown, f"{relation} has no column(s) {unknown}; S5 declares {columns}"
    connection.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.{relation} ({', '.join(columns)}) "
        f"VALUES ({', '.join('?' for _ in columns)})",
        [values.get(column) for column in columns],
    )


def insert_edge(
    connection: duckdb.DuckDBPyConnection, pair: tuple[str, str], *, run_id: str
) -> None:
    """One active `match_scores` row above the cut (S5), canonical per S5.0."""
    rec_a_key, rec_b_key = pair
    assert rec_a_key < rec_b_key, f"({rec_a_key!r}, {rec_b_key!r}) is not canonical (S5.0)"
    insert_row(
        connection,
        "match_scores",
        {
            "rec_a_key": rec_a_key,
            "rec_b_key": rec_b_key,
            "match_probability": SCORED_PROBABILITY,
            "model_version": MODEL_VERSION,
            "tf_snapshot_id": TF_SNAPSHOT_ID,
            "rec_a_content_hash": f"{rec_a_key}-hash",
            "rec_b_content_hash": f"{rec_b_key}-hash",
            "evidence": "{}",
            "is_active": True,
            "run_id": run_id,
            "scored_at": SCORED_AT,
        },
    )


def chain_edges(*chains: tuple[str, ...]) -> list[tuple[str, str]]:
    """The consecutive pairs of every chain, canonicalised by construction."""
    return [(chain[index], chain[index + 1]) for chain in chains for index in range(len(chain) - 1)]


def standardized_keys(connection: duckdb.DuckDBPyConnection) -> frozenset[str]:
    """Every `record_key` the standardized corpus currently holds (S4.2)."""
    rows = connection.execute(f"SELECT record_key FROM {STD_RECORDS}").fetchall()
    return frozenset(str(key) for (key,) in rows)


def relations_in_lake(connection: duckdb.DuckDBPyConnection) -> frozenset[str]:
    """Every relation live in `lake.main` right now.

    Split out of :data:`~er.lake.model.SCHEMA_QUALIFIER` rather than written as two
    literals, so the alias this asserts about is the one every statement in `src/er` is
    qualified with.
    """
    rows = connection.execute(
        "SELECT table_name FROM duckdb_tables() WHERE database_name = ? AND schema_name = ?",
        list(SCHEMA_QUALIFIER.split(".", 1)),
    ).fetchall()
    return frozenset(str(name) for (name,) in rows)


def reference_components(nodes: frozenset[str], edges: list[Edge]) -> dict[str, frozenset[str]]:
    """The connected components of the subgraph, by breadth-first search.

    The independent implementation AC6 compares against, and independent on purpose: it is
    a BFS over an adjacency map, not the union-find the failure path of
    `er.entities.cluster` uses and not label propagation at all. Two implementations that
    shared a trick would agree about it while both being wrong, which is precisely what
    S4.5.2 says T-INV-1 exists to prevent.

    Returns:
        `component minimum -> members`, which is the shape S4.5.2's labelling has: the
        label of a component IS its minimum `record_key`.
    """
    adjacency: dict[str, set[str]] = {key: set() for key in nodes}
    for edge in edges:
        adjacency[edge.rec_a_key].add(edge.rec_b_key)
        adjacency[edge.rec_b_key].add(edge.rec_a_key)

    seen: set[str] = set()
    components: dict[str, frozenset[str]] = {}
    for start in sorted(nodes):
        if start in seen:
            continue
        members: set[str] = set()
        queue = deque([start])
        seen.add(start)
        while queue:
            node = queue.popleft()
            members.add(node)
            for neighbour in sorted(adjacency[node]):
                if neighbour not in seen:
                    seen.add(neighbour)
                    queue.append(neighbour)
        components[min(members)] = frozenset(members)
    return components


def partition_of(labels: Mapping[str, str]) -> dict[str, frozenset[str]]:
    """The labelling as `label -> members`, which is the partition it means."""
    grouped: dict[str, set[str]] = {}
    for key, label in labels.items():
        grouped.setdefault(label, set()).add(key)
    return {label: frozenset(members) for label, members in grouped.items()}


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


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return config()


@pytest.fixture
def scenario() -> Scenario:
    """The S8.2 fixture this suite ingests, opened through ER-028's loader."""
    loaded_scenario = load_scenario(SCENARIO_NAME)
    assert loaded_scenario.phases == (PHASE,)
    return loaded_scenario


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


@dataclass(frozen=True)
class Subgraph:
    """`base_10`'s affected subgraph, through the real S4.5.1 chain."""

    connection: duckdb.DuckDBPyConnection
    affected: AffectedSet
    edges: list[Edge]
    max_iterations: int


@pytest.fixture
def subgraph(
    scenario: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[Subgraph]:
    """`base_10` ingested, standardized, scored by hand, and reduced to its subgraph.

    The node set comes from :func:`~er.entities.cluster.affected_nodes` and the edges from
    :func:`~er.entities.cluster.affected_edges` rather than from literals, so what the loop
    is asked to propagate over is what the stage will hand it. The seed is the whole
    standardized corpus, which is what S4.5.1's batch arm yields on a first run: every
    record of `base_10` was delivered by it.

    Function-scoped because `clean_lake` drops the dbt-owned relations between tests
    (S8.1); `int_std_records` has to exist for the edge set's endpoint restriction to mean
    anything.
    """
    root = tmp_path / "drop"
    for source, path in scenario.inputs_for(PHASE).items():
        deliver({path.name: path.read_text(encoding="utf-8")}, root, source)
        ingest(source, root)

    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")
    dbt.standardize()

    corpus = standardized_keys(initialised_lake)
    assert {*P1_CHAIN, *P2_CHAIN, *P8_PAIR, ISOLATED} <= corpus

    run_id = str(ULID())
    for pair in chain_edges(P1_CHAIN, P2_CHAIN, P8_PAIR):
        insert_edge(initialised_lake, pair, run_id=run_id)

    affected = affected_nodes(
        corpus,
        edges=list(
            current_edges(
                initialised_lake, MODEL_VERSION, TF_SNAPSHOT_ID, min_probability=AUTO_MERGE
            )
        ),
        membership={},
        auto_merge=AUTO_MERGE,
    )
    edges = affected_edges(
        initialised_lake,
        affected.nodes,
        model_version=MODEL_VERSION,
        tf_snapshot_id=TF_SNAPSHOT_ID,
        auto_merge=AUTO_MERGE,
    )
    assert affected.nodes == corpus, "a first run's affected set is the whole corpus"
    assert len(edges) == len(chain_edges(P1_CHAIN, P2_CHAIN, P8_PAIR))

    yield Subgraph(
        connection=initialised_lake,
        affected=affected,
        edges=edges,
        max_iterations=cfg.clustering.max_iterations,
    )


def test_components_match_reference_on_base_10(subgraph: Subgraph) -> None:
    """AC6: the labelling is the connected components, and no Splink relation leaked.

    The comparison S4.5.2 makes T-INV-1 out of, one stage earlier and against a reference
    written a different way (BFS, in this module). Both sides are computed over the SAME
    edge set — the one `affected_edges` returned — so a disagreement can only be the
    propagation, which is the point: this is the guard against the incremental loop and
    the connected components it is supposed to equal drifting apart.

    The two chains are what make it a test of propagation rather than of a single hop:
    `webforms:W002` is three edges from its component's minimum, so a label has to travel.
    """
    result = label_propagate(
        subgraph.connection,
        subgraph.affected.nodes,
        [edge.pair for edge in subgraph.edges],
        max_iterations=subgraph.max_iterations,
    )

    expected = reference_components(subgraph.affected.nodes, subgraph.edges)
    assert partition_of(result.labels) == expected
    assert set(result.labels) == set(subgraph.affected.nodes), (
        "every affected node is labelled; a dropped record reaches no reconciler"
    )
    assert result.labels[P1_CHAIN[-1]] == P1_CHAIN[0], (
        "the far end of the chain carries the component minimum, which is the billing key"
    )
    assert result.labels[ISOLATED] == ISOLATED, "an unedged record is its own singleton"
    assert expected[P1_CHAIN[0]] == frozenset(P1_CHAIN)
    assert result.iterations > 1, (
        "a subgraph with a chain of four settled in one round; the graph under test is "
        "not exercising the propagation"
    )
    assert result.iterations <= MAX_ITERATION_BOUND(len(subgraph.affected.nodes))
    assert result.counters == {LABEL_PROP_ITERATIONS: result.iterations}

    assert_no_splink_relations_in_lake(subgraph.connection)


def test_loop_writes_no_lake_relation_and_no_splink_scratch(subgraph: Subgraph) -> None:
    """AC7: `lake.main`'s relations and the DuckLake snapshot are unchanged by the loop.

    S4.0b's rule for this stage, stated as its consequence: "iterations run in the
    in-memory database ... so the loop cannot commit one snapshot per iteration". A loop
    that materialised its state in `lake.main` would commit one snapshot per round between
    the endpoints `run_stages` records for the stage (S5.2), and S4.7's recovery story —
    time travel over that range — would be reading a partial partition.

    Three assertions rather than one, because a leak can take three shapes: a relation that
    is still there when the call returns, a relation that was created and dropped (which
    leaves the relation set identical and the snapshot moved), and a `__splink__` scratch
    relation, which is M17's arm of the same rule.
    """
    before = relations_in_lake(subgraph.connection)
    snapshot_before = current_snapshot(subgraph.connection)

    result = label_propagate(
        subgraph.connection,
        subgraph.affected.nodes,
        [edge.pair for edge in subgraph.edges],
        max_iterations=subgraph.max_iterations,
    )

    assert relations_in_lake(subgraph.connection) == before, (
        "the propagation changed the relation set of lake.main; the loop runs in the "
        "in-memory database and writes nothing (S4.0b, M17)"
    )
    assert current_snapshot(subgraph.connection) == snapshot_before, (
        "the propagation committed a DuckLake snapshot; S4.0b forbids one per iteration "
        "and this loop commits none at all"
    )
    assert_no_splink_relations_in_lake(subgraph.connection)
    assert not [name for name in before if name.startswith("er_label_prop")], (
        "a loop relation is in lake.main; its tables are TEMP and live in the in-memory database"
    )
    assert result.labels, "the labelling is the one thing that does leave the loop"
