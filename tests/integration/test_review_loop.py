"""T-REVIEW-1/2: the review loop, gray band to merge, end to end (S4.3.5, S4.4, S8.3).

M20's complaint was that `review_queue` is written and never read: no tested path
led from a gray-band score to a steward decision to a merge. This module walks that
path on `base_10`'s single designed gray-band pair — cross-persona by construction
(S8.2), so its resolution legitimately merges two personas and leaves nine
entities; that is the steward's call being honoured, not a quality regression, and
no cluster metric is asserted here.

The load-bearing test is the resolution-driven merge with ZERO new records: the
only way `er reconcile` can reach the pair is through S4.5.1's assertion-delta and
review-resolution arms of the affected-node seed. The merge is therefore driven
through the CLI — the seed is the thing under test — and a failure here is a seed
defect before it is a reconciler one.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator, Mapping
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import fixture_settings, load_fixture_model
from helpers.scenario import Scenario, load_scenario
from helpers.traps import load_trap_index
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.ids import canonicalize_pair, resolve
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.full import score_full
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.obs.runctx import DECLARED_COUNTERS, StageCounters, StageRun
from er.review.queue import BAYES_FACTOR_PREFIX, GAMMA_PREFIX

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"
TRAPS_FILE: Final = "traps.csv"
GRAY_BAND_TRAP: Final = "gray_band_pair"

MATCH_STAGE: Final = "match"
MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.review_queue"
ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.assertions"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
ENTITIES: Final = f"{SCHEMA_QUALIFIER}.entities"

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


class Loop:
    """`base_10` with a scoring pass available on demand, one run id per pass."""

    def __init__(
        self,
        connection: duckdb.DuckDBPyConnection,
        cfg: Config,
        model_version: str,
        tf_snapshot_id: str,
    ) -> None:
        self.connection = connection
        self.cfg = cfg
        self.model_version = model_version
        self.tf_snapshot_id = tf_snapshot_id
        self.run_ids: list[str] = []

    def score_and_reconcile(self, *, reconcile: bool = True) -> str:
        """One full scoring pass — what `er run-all --mode full` amounts to here."""
        run_id = str(ULID())
        self.run_ids.append(run_id)
        stage_run = StageRun(
            run_id=run_id,
            stage=MATCH_STAGE,
            seq=1,
            started_at=datetime.now(UTC),
            counters=StageCounters(DECLARED_COUNTERS[MATCH_STAGE]),
        )
        result = score_full(
            self.connection,
            self.cfg,
            stage_run,
            model_version=self.model_version,
            tf_snapshot_id=self.tf_snapshot_id,
            settings=fixture_settings(),
        )
        assert result.pairs_scored > 0
        if reconcile:
            settled = run_er("reconcile", "--run-id", run_id)
            assert settled.returncode in (
                int(ExitCode.SUCCESS),
                int(ExitCode.NOTHING_TO_DO),
            ), settled.stdout + settled.stderr
        return run_id

    def gray_rows(self) -> list[dict[str, Any]]:
        rows = self.connection.execute(
            f"SELECT review_id, rec_a_key, rec_b_key, subject_type, status, "
            f"match_probability, first_seen_run_id, last_seen_run_id, waterfall, "
            f"resolved_by, resolved_at "
            f"FROM {REVIEW_QUEUE} WHERE reason = 'gray_band' ORDER BY review_id"
        ).fetchall()
        names = (
            "review_id",
            "rec_a_key",
            "rec_b_key",
            "subject_type",
            "status",
            "match_probability",
            "first_seen_run_id",
            "last_seen_run_id",
            "waterfall",
            "resolved_by",
            "resolved_at",
        )
        return [dict(zip(names, row, strict=True)) for row in rows]

    def entity_of(self, record_key: str) -> str:
        return str(
            scalar(
                self.connection,
                f"SELECT entity_id FROM {MEMBERSHIP} WHERE record_key = ?",
                record_key,
            )
        )


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return config()


@pytest.fixture
def gray_pair() -> tuple[str, str]:
    """The single designed pair, from the committed trap index (S8.2)."""
    scenario = load_scenario(SCENARIO_NAME)
    keys = load_trap_index(scenario.truth[TRAPS_FILE])[GRAY_BAND_TRAP]
    assert len(keys) == 2
    return canonicalize_pair(keys[0], keys[1])


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
def loop(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[Loop]:
    """`base_10` scored and reconciled once — the state every arm starts from."""
    scenario = load_scenario(SCENARIO_NAME)
    run_id = str(ULID())
    delivery = deliver(scenario, tmp_path / "drop")
    for source in scenario.inputs_for(PHASE):
        result = run_er("ingest", "--source", source, "--path", str(delivery), "--run-id", run_id)
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")
    dbt.standardize()

    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        model_version, tf_snapshot_id, _ = load_fixture_model(initialised_lake)
        driven = Loop(initialised_lake, cfg, model_version, tf_snapshot_id)
        driven.run_ids.append(run_id)
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
        assert result.pairs_scored > 0
        settled = run_er("reconcile", "--run-id", run_id)
        assert settled.returncode == int(ExitCode.SUCCESS), settled.stdout + settled.stderr
        yield driven
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def _resolve_as_match(loop: Loop, review_id: str) -> None:
    resolved = run_er(
        "review", "resolve", "--review-id", review_id, "--as", "match", "--by", "tester"
    )
    assert resolved.returncode == int(ExitCode.SUCCESS), resolved.stdout + resolved.stderr


def test_gray_band_pair_lands_open(loop: Loop, gray_pair: tuple[str, str], cfg: Config) -> None:
    """T-REVIEW-1 / AC1-AC3: one open row, full evidence, pair not clustered."""
    rows = loop.gray_rows()
    assert len(rows) == 1, f"{len(rows)} gray_band rows; base_10 designs exactly one (S8.2)"
    row = rows[0]
    assert (str(row["rec_a_key"]), str(row["rec_b_key"])) == gray_pair
    assert str(row["subject_type"]) == "pair"
    assert str(row["status"]) == "open"
    assert str(row["rec_a_key"]) < str(row["rec_b_key"])
    assert (
        cfg.thresholds.review_low <= float(row["match_probability"]) < (cfg.thresholds.auto_merge)
    )
    assert str(row["first_seen_run_id"]) == str(row["last_seen_run_id"])

    # AC2: the waterfall names EVERY configured comparison, both key families —
    # enumerated from the config so a projected-away column fails by name.
    waterfall: Mapping[str, Any] = json.loads(str(row["waterfall"]))
    for comparison in cfg.comparisons:
        assert f"{GAMMA_PREFIX}{comparison}" in waterfall, (
            f"waterfall lacks {GAMMA_PREFIX}{comparison}; S4.3.5 retains the full "
            f"gamma vector: {sorted(waterfall)}"
        )
        assert f"{BAYES_FACTOR_PREFIX}{comparison}" in waterfall, (
            f"waterfall lacks {BAYES_FACTOR_PREFIX}{comparison} (S4.3.5)"
        )

    assert loop.entity_of(gray_pair[0]) != loop.entity_of(gray_pair[1]), (
        "the gray-band pair was clustered before any steward decided it (S4.3)"
    )


def test_second_run_refreshes_last_seen_only(loop: Loop) -> None:
    """AC4: idempotent refresh — one row, new last_seen, unchanged first_seen."""
    first_seen = str(loop.gray_rows()[0]["first_seen_run_id"])
    second = loop.score_and_reconcile()

    rows = loop.gray_rows()
    assert len(rows) == 1, f"the second run inserted a duplicate: {rows}"
    assert str(rows[0]["last_seen_run_id"]) == second
    assert str(rows[0]["first_seen_run_id"]) == first_seen


def test_resolution_triggers_merge_without_new_records(
    loop: Loop, gray_pair: tuple[str, str]
) -> None:
    """T-REVIEW-2 / AC5, AC6, AC8: resolve -> assertion -> zero-ingest merge."""
    row = loop.gray_rows()[0]
    review_id = str(row["review_id"])
    entity_a = loop.entity_of(gray_pair[0])
    entity_b = loop.entity_of(gray_pair[1])
    assert entity_a != entity_b

    # AC8 first, against the untouched row: an unknown id writes nothing.
    unknown = run_er(
        "review", "resolve", "--review-id", str(ULID()), "--as", "match", "--by", "tester"
    )
    assert unknown.returncode == 2, unknown.stdout + unknown.stderr
    assert str(loop.gray_rows()[0]["status"]) == "open"

    _resolve_as_match(loop, review_id)

    resolved = loop.gray_rows()[0]
    assert str(resolved["status"]) == "resolved_match"
    assert str(resolved["resolved_by"]) == "tester"
    assert resolved["resolved_at"] is not None
    always = loop.connection.execute(
        f"SELECT kind, active FROM {ASSERTIONS} WHERE rec_a_key = ? AND rec_b_key = ?",
        list(gray_pair),
    ).fetchall()
    assert [(str(kind), bool(active)) for kind, active in always] == [("always", True)], (
        f"resolution wrote {always}; S4.3.5 writes the always row in the same "
        "transaction as the status change"
    )

    # AC6: the merge arrives with ZERO new records — the seed's assertion-delta
    # and review-resolution arms are the only route to the pair (S4.5.1).
    run_id = str(ULID())
    merged = run_er("reconcile", "--run-id", run_id)
    assert merged.returncode == int(ExitCode.SUCCESS), merged.stdout + merged.stderr
    assert loop.entity_of(gray_pair[0]) == loop.entity_of(gray_pair[1]), (
        "the resolved pair did not merge; the affected-node seed is not reading "
        "the assertion/review deltas (S4.5.1)"
    )
    merged_events = loop.connection.execute(
        f"SELECT entity_id FROM {EVENTS} WHERE run_id = ? AND event_type = 'merged'",
        [run_id],
    ).fetchall()
    assert len(merged_events) == 1, f"{len(merged_events)} merged events for the run"
    loser = str(merged_events[0][0])
    survivor = loop.entity_of(gray_pair[0])
    redirects = {
        str(entity): str(target)
        for entity, target in loop.connection.execute(
            f"SELECT entity_id, merged_into FROM {ENTITIES} WHERE merged_into IS NOT NULL"
        ).fetchall()
    }
    assert resolve(loser, redirects) == survivor, (
        "ids.resolve did not follow the merged-away entity to the survivor"
    )


def test_resolved_pair_does_not_reopen(loop: Loop, gray_pair: tuple[str, str]) -> None:
    """AC7: a later run re-scores the pair in the band and inserts nothing."""
    review_id = str(loop.gray_rows()[0]["review_id"])
    _resolve_as_match(loop, review_id)
    run_id = str(ULID())
    merged = run_er("reconcile", "--run-id", run_id)
    assert merged.returncode == int(ExitCode.SUCCESS), merged.stdout + merged.stderr

    loop.score_and_reconcile()

    rows = loop.gray_rows()
    assert len(rows) == 1, (
        f"a later run gave the resolved pair a second row: {rows}. The skip rule "
        "for resolved pairs is what stops a dismissed decision resurfacing every run"
    )
    assert str(rows[0]["status"]) == "resolved_match"
