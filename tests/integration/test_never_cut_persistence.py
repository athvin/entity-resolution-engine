"""A cut outlives the run that made it, and dies with its assertion (S4.4.2).

The pure cut search is exercised in `tests/unit/review/test_never_cut.py`. What only a
lake can show is the half S4.4.2 spends a whole paragraph on:

    "`cut_edges` rows are excluded from the clustering edge set on every subsequent
    run. Without that exclusion, every cut is silently re-merged on the next run from
    the cumulative `match_scores` table, and `never` becomes a no-op with a one-run
    half-life."

That failure is invisible in a single run — the first reconcile cuts correctly and the
partition looks right — so the arm that matters is the *second* run over an unchanged
corpus. Likewise a release: retracting the `never` must let the component re-merge, and
nothing but a second run can show it.

The third arm is T-INV-1 over a cut scenario. It is not redundant with the finalizer
that runs anyway: the finalizer would fail the *whole suite* if the helper recomputed
components over the pre-cut edge set, and a named test is what tells the next reader
which property broke rather than which scenario happened to run last.

`never(a, b)` is asserted over a pair `base_10` genuinely co-clusters, so the cut has
something to separate. The pair is read from the membership the first reconcile wrote
rather than hard-coded, because which records share an entity is a property of the
model and ER-060 has already shown those can move.
"""

from __future__ import annotations

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
from er.matching.full import score_full
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.obs.counters import DECLARED_COUNTERS, StageCounters
from er.obs.runctx import StageRun

SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
CUT_EDGES: Final = f"{SCHEMA_QUALIFIER}.cut_edges"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.assertions"

MATCH_STAGE: Final = "match"
EDGE_CUT: Final = "edge_cut"
STEWARD: Final = "er-076"


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
    """Materialise the base phase as the drop-folder root `er ingest --path` reads."""
    for source, path in scenario.inputs_for(PHASE).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def entity_of(connection: duckdb.DuckDBPyConnection, record: str) -> str | None:
    """The entity a record currently belongs to, or ``None`` if it holds no row."""
    row = connection.execute(
        f"SELECT entity_id FROM {MEMBERSHIP} WHERE record_key = ?", [record]
    ).fetchone()
    return None if row is None else str(row[0])


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
class Cut:
    """`base_10` reconciled once, plus a co-clustered pair to assert `never` over."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    pair: tuple[str, str]
    entity: str


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
def reconciled(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[Cut]:
    """`base_10` ingested, standardized, scored and reconciled once.

    The delivery and the reconcile share one run: S4.5.1's batch arm is scoped by
    `run_id` through `ingest_batches`, which is what `er run-all` does when it chains
    the two.
    """
    scenario = load_scenario(SCENARIO_NAME)
    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")
    delivery = deliver(scenario, tmp_path / "drop")
    ingest_run = str(ULID())
    for source in scenario.inputs_for(PHASE):
        result = run_er(
            "ingest", "--source", source, "--path", str(delivery), "--run-id", ingest_run
        )
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
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
        first = run_er("reconcile", "--run-id", ingest_run)
        assert first.returncode == int(ExitCode.SUCCESS), first.stdout + first.stderr

        # A pair `base_10` genuinely co-clusters, read from the membership rather than
        # hard-coded: which records share an entity is a property of the model, and
        # ER-060 has shown those can move.
        row = initialised_lake.execute(
            f"SELECT a.record_key, b.record_key, a.entity_id FROM {MEMBERSHIP} AS a "
            f"JOIN {MEMBERSHIP} AS b ON a.entity_id = b.entity_id AND a.record_key < b.record_key "
            "ORDER BY a.record_key, b.record_key LIMIT 1"
        ).fetchone()
        assert row is not None, "no entity holds two records; there is nothing to cut apart"
        yield Cut(
            connection=initialised_lake,
            cfg=cfg,
            pair=(str(row[0]), str(row[1])),
            entity=str(row[2]),
        )
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def _assert_never(pair: tuple[str, str]) -> str:
    """Add `never(pair)` through the real console script and return its assertion id."""
    left, right = pair
    added = run_er("assert", "add", "--kind", "never", "--a", left, "--b", right, "--by", STEWARD)
    assert added.returncode == int(ExitCode.SUCCESS), added.stdout + added.stderr
    return added.stdout.strip()


def test_cut_row_persists_and_is_excluded_next_run(reconciled: Cut) -> None:
    """AC6: one active cut row, and a second run neither re-cuts nor re-merges.

    The second run is the whole test. Without S4.4.2's exclusion the cut is silently
    re-merged from the cumulative `match_scores` table and `never` has a one-run
    half-life — a failure the first run cannot show, because the first run is correct.
    """
    left, right = reconciled.pair
    _assert_never(reconciled.pair)

    cutting = run_er("reconcile", "--run-id", str(ULID()))
    assert cutting.returncode == int(ExitCode.SUCCESS), cutting.stdout + cutting.stderr

    rows = reconciled.connection.execute(
        f"SELECT rec_a_key, rec_b_key, match_probability, assertion_id, cut_run_id, active "
        f"FROM {CUT_EDGES} WHERE active"
    ).fetchall()
    assert len(rows) == 1, f"expected exactly one active cut row, got {rows}"
    rec_a, rec_b, probability, assertion_id, cut_run_id, active = rows[0]
    assert str(rec_a) < str(rec_b), "the cut pair is not canonical (S5.0)"
    assert float(probability) > 0.0, "the cut row records no probability at cut time"
    assert assertion_id is not None, "the cut names no assertion (S5)"
    assert cut_run_id is not None, "the run of a cut goes in cut_run_id, not run_id (S5)"
    assert bool(active) is True

    assert entity_of(reconciled.connection, left) != entity_of(reconciled.connection, right), (
        f"{left} and {right} are still co-clustered after the cut; a partition-level "
        "never means no path survives (S4.4.2)"
    )
    cut_events = int(
        scalar(
            reconciled.connection, f"SELECT count(*) FROM {EVENTS} WHERE event_type = ?", EDGE_CUT
        )
    )
    assert cut_events >= 1, "no edge_cut event was emitted for the cut (S4.4.2 step 5)"

    # The second run over an unchanged corpus: no second cut row, no second event, and
    # crucially the pair stays apart.
    again = run_er("reconcile", "--run-id", str(ULID()))
    assert again.returncode in (int(ExitCode.SUCCESS), int(ExitCode.NOTHING_TO_DO)), (
        again.stdout + again.stderr
    )
    assert int(scalar(reconciled.connection, f"SELECT count(*) FROM {CUT_EDGES}")) == 1, (
        "the second run wrote another cut row; a cut is kept until its assertion is "
        "retracted, so an unchanged corpus adds nothing (S4.4.2)"
    )
    assert (
        int(
            scalar(
                reconciled.connection,
                f"SELECT count(*) FROM {EVENTS} WHERE event_type = ?",
                EDGE_CUT,
            )
        )
        == cut_events
    ), "the second run emitted another edge_cut for a cut that was already made"
    assert entity_of(reconciled.connection, left) != entity_of(reconciled.connection, right), (
        "the second run RE-MERGED the cut pair. This is exactly the one-run half-life "
        "S4.4.2 warns about: the cut must be excluded from the clustering edge set on "
        "every subsequent run"
    )


def test_retraction_releases_cut_and_remerges(reconciled: Cut) -> None:
    """AC7: retracting the `never` releases the row and the component re-merges."""
    left, right = reconciled.pair
    _assert_never(reconciled.pair)
    cutting = run_er("reconcile", "--run-id", str(ULID()))
    assert cutting.returncode == int(ExitCode.SUCCESS), cutting.stdout + cutting.stderr
    assert entity_of(reconciled.connection, left) != entity_of(reconciled.connection, right)

    assertion_id = str(
        scalar(
            reconciled.connection,
            f"SELECT assertion_id FROM {ASSERTIONS} WHERE active AND kind = 'never' "
            "AND rec_a_key = ? AND rec_b_key = ?",
            left,
            right,
        )
    )
    retracted = run_er("assert", "remove", "--assertion-id", assertion_id, "--by", STEWARD)
    assert retracted.returncode == int(ExitCode.SUCCESS), retracted.stdout + retracted.stderr

    released_run = str(ULID())
    remerge = run_er("reconcile", "--run-id", released_run)
    assert remerge.returncode == int(ExitCode.SUCCESS), remerge.stdout + remerge.stderr

    row = reconciled.connection.execute(
        f"SELECT active, released_run_id, released_at FROM {CUT_EDGES}"
    ).fetchone()
    assert row is not None, "the cut row disappeared; a release is a stamp, not a delete"
    active, released_run_id, released_at = row
    assert bool(active) is False, "the retracted assertion did not deactivate its cut"
    assert released_run_id is not None and released_at is not None, (
        "the release recorded no run or stamp (S5)"
    )

    assert entity_of(reconciled.connection, left) == entity_of(reconciled.connection, right), (
        f"{left} and {right} did not re-merge after the never was retracted; a released "
        "cut must stop excluding its edge (S4.4.2)"
    )


def test_invariant_holds_against_post_cut_edge_set(reconciled: Cut) -> None:
    """AC8: T-INV-1 passes on a scenario containing a cut.

    Named rather than left to the autouse finalizer: the finalizer would fail every
    scenario if the helper recomputed components over the pre-cut edge set, and a
    reader of that failure would have no way to tell which property broke.
    """
    _assert_never(reconciled.pair)
    cutting = run_er("reconcile", "--run-id", str(ULID()))
    assert cutting.returncode == int(ExitCode.SUCCESS), cutting.stdout + cutting.stderr
    assert int(scalar(reconciled.connection, f"SELECT count(*) FROM {CUT_EDGES} WHERE active")) == 1

    assert_membership_equals_components(reconciled.connection)
