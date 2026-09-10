"""T-MATCH-1a: edge-level quality on `base_10`, as absolute counts (S8.3, S8.5, S8.2).

Every number here comes from `er.eval.pairwise_metrics` — the S8.5 single
implementation — fed the three set triples `tests/helpers/quality.py` builds. The
module gates rather than reports: blocking recall must be exactly `1.0` over the
full `C(23,2) = 253` universe, because it is the only number that can fall when a
blocking rule stops emitting a key (edge-level metrics are computed over the
blocked universe and lose the pair from both sides). At `auto_merge` the gate is
absolute — zero false-positive pairs, at most one missed true pair — and a
tolerated miss must satisfy S8.2's authoring constraint: inside a persona of three
or more records whose remaining above-`auto_merge` edges stay connected, or the
tolerance is masking exactly the entity-count failure it exists to prevent.

The lint's falsifiability test lives here rather than in the unit suite because it
runs the real `scripts/lint_metrics.py` as a subprocess against a planted second
implementation — proof the S8.5 "exactly one implementation" rule is a gate, in
the same environment the static job runs it.
"""

from __future__ import annotations

import os
import subprocess
import sys
from collections.abc import Iterator, Mapping
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import fixture_settings, load_fixture_model
from helpers.quality import all_pairs_universe, blocked_universe, predicted_edges_at, truth_pairs
from helpers.scenario import Scenario, load_scenario
from helpers.traps import Pair, load_trap_index, persona_members, persona_of_record
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.ids import canonicalize_pair
from er.errors import ExitCode
from er.eval.metrics import pairwise_metrics
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
MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"

#: S8.2's ground truth, machine-checked upstream: 23 records over 10 personas give
#: 18 true pairs in a universe of C(23,2) = 253. Asserted here as a precondition so
#: a truth-file edit fails this suite by name rather than by a drifted ratio.
EXPECTED_RECORDS: Final = 23
EXPECTED_TRUE_PAIRS: Final = 18
EXPECTED_UNIVERSE: Final = 253

#: The trap names whose records AC6's four sub-assertions are stated over (S8.2).
NICKNAME_PAIR: Final = "nickname_pair"
TYPO_SURNAME: Final = "typo_surname"
SHARED_HOUSEHOLD: Final = "shared_household"
PLACEHOLDER_EMAIL: Final = "placeholder_email"

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
    """The committed `truth.csv` — the ground truth every metric is graded against."""
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
def standardized(
    base_10: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[duckdb.DuckDBPyConnection]:
    """`base_10` ingested and standardized on this test's fresh namespace."""
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
        yield initialised_lake
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


@pytest.fixture
def scored(standardized: duckdb.DuckDBPyConnection, cfg: Config) -> duckdb.DuckDBPyConnection:
    """`base_10` scored once at the committed model."""
    model_version, tf_snapshot_id, _ = load_fixture_model(standardized)
    stage_run = StageRun(
        run_id=str(ULID()),
        stage=MATCH_STAGE,
        seq=1,
        started_at=datetime.now(UTC),
        counters=StageCounters(DECLARED_COUNTERS[MATCH_STAGE]),
    )
    result = score_full(
        standardized,
        cfg,
        stage_run,
        model_version=model_version,
        tf_snapshot_id=tf_snapshot_id,
        settings=fixture_settings(),
    )
    assert result.pairs_scored > 0, "the stage scored nothing; every metric below is vacuous"
    return standardized


def trap_pair(traps: Mapping[str, tuple[str, ...]], name: str) -> Pair:
    """The canonical pair a two-record trap names."""
    keys = traps[name]
    assert len(keys) == 2, f"{name} indexes {len(keys)} records; a pair trap indexes two"
    return canonicalize_pair(keys[0], keys[1])


def connected(members: tuple[str, ...], edges: set[Pair]) -> bool:
    """Whether ``edges`` connect every member — S8.2's transitive-recovery test."""
    if not members:
        return True
    reached = {members[0]}
    frontier = [members[0]]
    while frontier:
        node = frontier.pop()
        for rec_a, rec_b in edges:
            for here, there in ((rec_a, rec_b), (rec_b, rec_a)):
                if here == node and there in set(members) and there not in reached:
                    reached.add(there)
                    frontier.append(there)
    return reached == set(members)


def test_blocking_recall_is_one(
    scored: duckdb.DuckDBPyConnection, truth: Path, cfg: Config
) -> None:
    """AC4: all 18 true pairs are candidates, over the full 253-pair universe."""
    universe = all_pairs_universe(truth)
    truths = truth_pairs(truth)
    assert len(universe) == EXPECTED_UNIVERSE, (
        f"the committed truth implies a universe of {len(universe)}, not "
        f"{EXPECTED_UNIVERSE}; base_10's 23/10/18 shape moved (S8.2)"
    )
    assert len(truths) == EXPECTED_TRUE_PAIRS

    metrics = pairwise_metrics(blocked_universe(scored), truths, universe)
    assert metrics.recall == 1.0, (
        f"blocking recall is {metrics.recall}: {metrics.fn} true pair(s) never became "
        "candidates. Edge-level metrics cannot see this — the pair is missing from "
        "their universe too — which is why this number gates (S8.5)"
    )


def test_edge_quality_base_10(scored: duckdb.DuckDBPyConnection, truth: Path, cfg: Config) -> None:
    """AC5 / T-MATCH-1a: zero false positives at `auto_merge`, at most one guarded miss."""
    universe = blocked_universe(scored)
    truths = truth_pairs(truth) & universe
    predicted = predicted_edges_at(scored, cfg.thresholds.auto_merge)
    metrics = pairwise_metrics(predicted, truths, universe)

    false_positives = sorted(predicted - truths)
    assert metrics.fp == 0, (
        f"{metrics.fp} false-positive pair(s) at auto_merge={cfg.thresholds.auto_merge}: "
        f"{false_positives}. T-MATCH-1a gates this at zero (S8.3)"
    )

    missed = sorted((truth_pairs(truth)) - predicted)
    assert len(missed) <= 1, (
        f"{len(missed)} true pairs are below auto_merge: {missed}; S8.3 tolerates one"
    )
    if missed:
        personas = persona_of_record(truth)
        members_by_persona = persona_members(truth)
        (rec_a, rec_b) = missed[0]
        assert personas[rec_a] == personas[rec_b]
        members = members_by_persona[personas[rec_a]]
        assert len(members) >= 3, (
            f"the missed pair {missed[0]} lies inside a {len(members)}-record persona; "
            "S8.2 forbids a tolerated miss outside a persona of three or more"
        )
        surviving = {pair for pair in predicted if pair != missed[0]}
        assert connected(members, surviving), (
            f"removing {missed[0]} disconnects persona {personas[rec_a]}; the miss is "
            "not transitively recoverable and T-MATCH-1b's entity count would break"
        )


def test_named_trap_subassertions(
    scored: duckdb.DuckDBPyConnection,
    traps: Mapping[str, tuple[str, ...]],
    cfg: Config,
) -> None:
    """AC6: the four S8.2 sub-assertions, by the records the trap index names."""
    at_auto = predicted_edges_at(scored, cfg.thresholds.auto_merge)
    ever_scored = predicted_edges_at(scored, 0.0)

    robert_bob = trap_pair(traps, NICKNAME_PAIR)
    assert robert_bob in at_auto, f"TRAP-{NICKNAME_PAIR}: {robert_bob} is below auto_merge"

    typo = trap_pair(traps, TYPO_SURNAME)
    assert typo in at_auto, f"TRAP-{TYPO_SURNAME}: {typo} is below auto_merge"

    household = trap_pair(traps, SHARED_HOUSEHOLD)
    assert household not in at_auto, (
        f"TRAP-{SHARED_HOUSEHOLD}: {household} merged; two people at one address are "
        "not one person (S8.2)"
    )

    placeholder = trap_pair(traps, PLACEHOLDER_EMAIL)
    assert placeholder not in ever_scored, (
        f"TRAP-{PLACEHOLDER_EMAIL}: {placeholder} holds a match_scores row; the nulled "
        "placeholder email must form no key and no edge at any probability (S8.2)"
    )


def test_lint_metrics_rejects_a_second_implementation(tmp_path: Path) -> None:
    """AC7: the S8.5 lint fails on a planted implementation and passes a clean tree."""
    script = REPO_ROOT / "scripts" / "lint_metrics.py"

    clean = tmp_path / "clean"
    (clean / "pkg").mkdir(parents=True)
    (clean / "pkg" / "fine.py").write_text(
        "def blocked_universe() -> set[tuple[str, str]]:\n    return set()\n",
        encoding="utf-8",
    )
    accepted = subprocess.run(
        [sys.executable, str(script), "--root", str(clean)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert accepted.returncode == 0, accepted.stderr

    planted = tmp_path / "planted"
    (planted / "pkg").mkdir(parents=True)
    (planted / "pkg" / "sneaky.py").write_text(
        "def edge_share(tp: int, fp: int) -> float:\n    return tp / (tp + fp)\n",
        encoding="utf-8",
    )
    rejected = subprocess.run(
        [sys.executable, str(script), "--root", str(planted)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert rejected.returncode != 0, (
        "a planted tp/(tp+fp) implementation passed scripts/lint_metrics.py; the "
        "S8.5 single-implementation rule is not being enforced"
    )
    assert "sneaky.py" in rejected.stderr

    named = tmp_path / "named"
    (named / "pkg").mkdir(parents=True)
    (named / "pkg" / "second.py").write_text(
        "def precision(hits: int, total: int) -> float:\n    return hits / total\n",
        encoding="utf-8",
    )
    by_name = subprocess.run(
        [sys.executable, str(script), "--root", str(named)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert by_name.returncode != 0, "a planted `def precision` passed the lint"
