"""T-MATCH-1b: cluster-level quality on `base_10` — the S8.5 headline (S8.3, S8.2).

Clusters, not edges, are the product: one bad edge chaining two 4-record clusters
costs one false pair at the edge level and sixteen here, which is why S8.5 calls
this row the headline and why its universe is the FULL `C(23,2)` set — a blocking
regression that removes a pair from the blocked universe cannot hide from a metric
that never restricted itself to it. The predicted set is the transitive closure of
`entity_membership` through `er.eval.metrics.cluster_closure_pairs`, and every
number is `pairwise_metrics`' (S8.5's single implementation; AC7 is the absence of
any arithmetic in this file).

`entity count == 10` is only satisfiable because of S8.2's two normative authoring
constraints — the gray-band pair is cross-persona, and a tolerated missed edge lies
inside a persona of three or more records — so a failure of that assertion is a
fixture-authoring conversation before it is a metric one.

The function name `test_cluster_quality_base_10` is pinned: ER-103 resolves S8.3's
T-MATCH-1b row onto it although the file path differs from the spec's sketch.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator, Mapping
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import fixture_settings, load_fixture_model
from helpers.quality import all_pairs_universe, truth_pairs
from helpers.scenario import Scenario, load_scenario
from helpers.traps import Pair, load_trap_index, persona_members
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.ids import canonicalize_pair
from er.errors import ExitCode
from er.eval.metrics import cluster_closure_pairs, membership_partition, pairwise_metrics
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.full import score_full
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.obs.runctx import DECLARED_COUNTERS, StageCounters, StageRun

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"
TRUTH_FILE: Final = "truth.csv"
TRAPS_FILE: Final = "traps.csv"

MATCH_STAGE: Final = "match"
MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"
REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.review_queue"

#: S8.3's gates, as the row states them: exact 1.0 precision, >= 17 of 18 pairs.
REQUIRED_PRECISION: Final = 1.0
REQUIRED_RECALL: Final = 0.94
EXPECTED_ENTITIES: Final = 10
EXPECTED_UNIVERSE: Final = 253
EXPECTED_TRUE_PAIRS: Final = 18

NICKNAME_PAIR: Final = "nickname_pair"
SHARED_HOUSEHOLD: Final = "shared_household"
PLACEHOLDER_EMAIL: Final = "placeholder_email"
GRAY_BAND_PAIR: Final = "gray_band_pair"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"


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


class Dbt:
    """dbt as a stage invokes it: real `--vars`, no connection spanning it (S4.0b)."""

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
def base_10() -> Scenario:
    """The S8.2 fixture, opened through ER-028's loader."""
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (PHASE,)
    return scenario


@pytest.fixture
def truth(base_10: Scenario) -> Path:
    """The committed `truth.csv` — never a recomputed labelling (AC of ER-041)."""
    return base_10.truth[TRUTH_FILE]


@pytest.fixture
def traps(base_10: Scenario) -> Mapping[str, tuple[str, ...]]:
    """The committed trap index: trap name -> the record keys constructing it."""
    return load_trap_index(base_10.truth[TRAPS_FILE])


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
def reconciled(
    base_10: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[duckdb.DuckDBPyConnection]:
    """`base_10` through the full chain: ingest, standardize, score, reconcile.

    One `run_id` threads the whole phase, exactly as `er run-all` would mint it:
    the S4.5.1 seed's first arm is "records in this run's ingest batches", so a
    reconcile under a different id than the ingest would find nothing affected.
    """
    run_id = str(ULID())
    delivery = deliver(base_10, tmp_path / "drop")
    for source in base_10.inputs_for(PHASE):
        result = run_er("ingest", "--source", source, "--path", str(delivery), "--run-id", run_id)
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")
    dbt.standardize()

    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        model_version, tf_snapshot_id, _ = load_fixture_model(initialised_lake)
        stage_run = StageRun(
            run_id=run_id,
            stage=MATCH_STAGE,
            seq=1,
            started_at=datetime.now(UTC),
            counters=StageCounters(DECLARED_COUNTERS[MATCH_STAGE]),
        )
        result = score_full(
            initialised_lake,
            cfg,
            stage_run,
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            settings=fixture_settings(),
        )
        assert result.pairs_scored > 0, "nothing scored; every closure below is empty"
        reconciled = run_er("reconcile", "--run-id", stage_run.run_id)
        assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr
        yield initialised_lake
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def membership_rows(connection: duckdb.DuckDBPyConnection) -> list[tuple[str, str]]:
    """`(record_key, entity_id)` — the closure's input, in one shape."""
    return [
        (str(record), str(entity))
        for record, entity in connection.execute(
            f"SELECT record_key, entity_id FROM {MEMBERSHIP}"
        ).fetchall()
    ]


def trap_pair(traps: Mapping[str, tuple[str, ...]], name: str) -> Pair:
    """The canonical pair a two-record trap names."""
    keys = traps[name]
    assert len(keys) == 2, f"{name} indexes {len(keys)} records; a pair trap indexes two"
    return canonicalize_pair(keys[0], keys[1])


def test_cluster_quality_base_10(
    reconciled: duckdb.DuckDBPyConnection, truth: Path, cfg: Config
) -> None:
    """AC1, AC2, AC8: the headline numbers over the full universe, non-vacuously."""
    universe = all_pairs_universe(truth)
    truths = truth_pairs(truth)
    assert len(universe) == EXPECTED_UNIVERSE
    assert len(truths) == EXPECTED_TRUE_PAIRS

    rows = membership_rows(reconciled)
    closure = cluster_closure_pairs(rows)
    metrics = pairwise_metrics(closure, truths, universe)
    assert metrics.precision == REQUIRED_PRECISION, (
        f"cluster precision is {metrics.precision}: {metrics.fp} false pair(s) are "
        f"co-clustered — one bad edge multiplies here, which is why this is the "
        f"headline (S8.5): {sorted(closure - truths)}"
    )
    assert metrics.recall >= REQUIRED_RECALL, (
        f"cluster recall {metrics.recall} < {REQUIRED_RECALL}: "
        f"{sorted(truths - closure)} are not co-clustered"
    )

    entities = int(scalar(reconciled, f"SELECT count(DISTINCT entity_id) FROM {MEMBERSHIP}"))
    assert entities == EXPECTED_ENTITIES, (
        f"{entities} entities, not {EXPECTED_ENTITIES}. Before touching the metric, "
        "re-read S8.2's two authoring constraints: a same-persona gray pair or a "
        "missed edge outside a 3+ persona produces exactly this failure"
    )

    # AC8: the assertion is falsifiable. Splitting one multi-record persona out of
    # the closure — the cluster-level effect of losing a load-bearing edge — must
    # push recall below the gate. Computed through the same single implementation,
    # over the same universe.
    largest = max(persona_members(truth).values(), key=len)
    assert len(largest) >= 3, "base_10 lost its 3+ personas; the perturbation is toothless"
    lost = {
        canonicalize_pair(a, b)
        for index, a in enumerate(sorted(largest))
        for b in sorted(largest)[index + 1 :]
    }
    perturbed = pairwise_metrics(closure - lost, truths, universe)
    assert perturbed.recall < REQUIRED_RECALL, (
        f"removing {len(lost)} true pair(s) left recall at {perturbed.recall}; the "
        "gate would tolerate a dissolved persona and asserts nothing"
    )


def test_designed_traps_at_cluster_level(
    reconciled: duckdb.DuckDBPyConnection,
    truth: Path,
    traps: Mapping[str, tuple[str, ...]],
    cfg: Config,
) -> None:
    """AC3-AC6: the four named S8.3 sub-assertions plus the gray-band precondition."""
    rows = membership_rows(reconciled)
    entity_of = dict(rows)
    closure = cluster_closure_pairs(rows)

    robert, bob = trap_pair(traps, NICKNAME_PAIR)
    assert entity_of[robert] == entity_of[bob], (
        f"TRAP-{NICKNAME_PAIR}: {robert} and {bob} occupy two entities; the seed-backed "
        "variant level failed to carry the pair"
    )

    left, right = trap_pair(traps, SHARED_HOUSEHOLD)
    assert entity_of[left] != entity_of[right], (
        f"TRAP-{SHARED_HOUSEHOLD}: two people at one address merged into one entity"
    )
    above_auto = int(
        scalar(
            reconciled,
            f"SELECT count(*) FROM {MATCH_SCORES} WHERE rec_a_key = ? AND rec_b_key = ? "
            "AND match_probability >= ?",
            left,
            right,
            cfg.thresholds.auto_merge,
        )
    )
    assert above_auto == 0, f"TRAP-{SHARED_HOUSEHOLD}: an auto_merge edge joins the household"

    test_a, test_b = trap_pair(traps, PLACEHOLDER_EMAIL)
    assert entity_of[test_a] != entity_of[test_b], (
        f"TRAP-{PLACEHOLDER_EMAIL}: the two test@test.com records share an entity"
    )
    assert (test_a, test_b) not in closure, (
        f"TRAP-{PLACEHOLDER_EMAIL}: the closure joins the placeholder pair"
    )

    # AC5: the singleton persona is its own entity of exactly one row.
    singleton_members = [keys for keys in persona_members(truth).values() if len(keys) == 1]
    assert len(singleton_members) == 1, "base_10 designs exactly one singleton persona (S8.2)"
    (singleton,) = singleton_members[0]
    peers = int(
        scalar(
            reconciled,
            f"SELECT count(*) FROM {MEMBERSHIP} WHERE entity_id = ?",
            entity_of[singleton],
        )
    )
    assert peers == 1, f"the singleton {singleton} shares its entity with {peers - 1} other(s)"

    # AC6: the gray-band pair is captured, not clustered — the precondition that
    # keeps the entity count at ten (S8.2, S4.3.5).
    gray_a, gray_b = trap_pair(traps, GRAY_BAND_PAIR)
    assert entity_of[gray_a] != entity_of[gray_b], (
        f"TRAP-{GRAY_BAND_PAIR}: the gray-band pair was clustered; the band exists so "
        "a steward decides it (S4.3.5)"
    )

    partition = membership_partition(rows)
    assert sum(len(members) for members in partition.values()) == len(rows)
