"""T-MODEL-1 and the S4.3.2 activation guard, end to end (S4.3.3, S4.7, S8.3).

The hazard is two probability scales against one threshold: rows scored under two
`(model_version, tf_snapshot_id)` generations, both at or above `review_low`, in
the current edge set one reconcile would cluster. The guard refuses that state
with exit ``3`` and `error_class='precondition'` BEFORE anything is written, and
the refusal names both generations so the operator's next command — `er match
--mode full` — is in the message.

T-MODEL-1's second `model_version` is allocated by REGISTRATION of the committed
fixture artifact, never by training (S8.3, S12 M3): v0002 carries v0001's exact
settings and frozen TF, so the rescored probabilities are identical, the partition
is genuinely set-equal, and `assert_ids_stable` is a claim about the lifecycle
rather than an artefact of degenerate EM over 23 records.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import fixture_settings, load_fixture_model
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.guards import scoring_generation_rows
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.full import score_full
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.obs.runctx import DECLARED_COUNTERS, StageCounters, StageRun

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

MATCH_STAGE: Final = "match"
MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"
TF_LOOKUP: Final = f"{SCHEMA_QUALIFIER}.tf_lookup"

SECOND_VERSION: Final = "v0002"
SECOND_SNAPSHOT: Final = "01JWMDTFSNAP00000000000002"

#: The S8.2 shared-household pair: never scored (no shared key), which is what
#: makes it the AC3 probe — an assertion edge with no `match_scores` row at all.
HOUSEHOLD_PAIR: Final = ("crm:C004", "webforms:W005")

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
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[duckdb.DuckDBPyConnection]:
    """`base_10` through the chain at v0001, with the settings published for `er match`."""
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
        import json as _json

        model_version, tf_snapshot_id, settings = load_fixture_model(initialised_lake)
        published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
        object_store.put_bytes(published, _json.dumps(settings).encode("utf-8"))
        initialised_lake.execute(
            f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
            [published, model_version],
        )
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
        first = run_er("reconcile", "--run-id", run_id)
        assert first.returncode == int(ExitCode.SUCCESS), first.stdout + first.stderr
        yield initialised_lake
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def membership_snapshot(connection: duckdb.DuckDBPyConnection) -> list[tuple[str, str]]:
    return [
        (str(record), str(entity))
        for record, entity in connection.execute(
            f"SELECT record_key, entity_id FROM {MEMBERSHIP} ORDER BY record_key"
        ).fetchall()
    ]


def _assert_refused_with_nothing_written(
    connection: duckdb.DuckDBPyConnection, run_id: str, *tokens: str
) -> None:
    """The S4.7 refusal shape: exit already checked; here the row and the silence."""
    error_class, detail = connection.execute(
        f"SELECT error_class, error_detail FROM {RUN_STAGES} "
        "WHERE run_id = ? AND stage = 'reconcile'",
        [run_id],
    ).fetchall()[0]
    assert str(error_class) == "precondition", f"error_class is {error_class!r}"
    for token in tokens:
        assert token in str(detail), f"{token!r} missing from error_detail: {detail}"
    events = int(scalar(connection, f"SELECT count(*) FROM {EVENTS} WHERE run_id = ?", run_id))
    assert events == 0, f"a refused reconcile emitted {events} event(s)"


def test_mixed_model_version_above_review_low_exits_3(
    reconciled: duckdb.DuckDBPyConnection,
) -> None:
    """AC1: a second model_version in the current edge set refuses the run."""
    before = membership_snapshot(reconciled)
    pair = reconciled.execute(
        f"SELECT rec_a_key, rec_b_key FROM {MATCH_SCORES} WHERE match_probability >= 0.95 LIMIT 1"
    ).fetchall()[0]
    # The hazard state, constructed surgically: one pair's current row re-stamped
    # as a second generation — exactly what a partial rescore at a newly activated
    # model leaves behind. DuckLake enforces nothing (S5.0), so the write succeeds.
    reconciled.execute(
        f"UPDATE {MATCH_SCORES} SET model_version = ? WHERE rec_a_key = ? AND rec_b_key = ?",
        [SECOND_VERSION, str(pair[0]), str(pair[1])],
    )

    run_id = str(ULID())
    refused = run_er("reconcile", "--run-id", run_id)
    assert refused.returncode == int(ExitCode.PRECONDITION), refused.stdout + refused.stderr
    assert "v0001" in refused.stderr and SECOND_VERSION in refused.stderr, refused.stderr
    _assert_refused_with_nothing_written(reconciled, run_id, "v0001", SECOND_VERSION)
    assert membership_snapshot(reconciled) == before, "a refused run rewrote membership"


def test_mixed_tf_snapshot_above_review_low_exits_3(
    reconciled: duckdb.DuckDBPyConnection,
) -> None:
    """AC2: the same hazard through the other half of the key (S4.3.3)."""
    before = membership_snapshot(reconciled)
    pair = reconciled.execute(
        f"SELECT rec_a_key, rec_b_key FROM {MATCH_SCORES} WHERE match_probability >= 0.95 LIMIT 1"
    ).fetchall()[0]
    reconciled.execute(
        f"UPDATE {MATCH_SCORES} SET tf_snapshot_id = ? WHERE rec_a_key = ? AND rec_b_key = ?",
        [SECOND_SNAPSHOT, str(pair[0]), str(pair[1])],
    )

    run_id = str(ULID())
    refused = run_er("reconcile", "--run-id", run_id)
    assert refused.returncode == int(ExitCode.PRECONDITION), refused.stdout + refused.stderr
    _assert_refused_with_nothing_written(reconciled, run_id, SECOND_SNAPSHOT)
    assert membership_snapshot(reconciled) == before


def test_assertion_edges_do_not_trip_the_guard(
    reconciled: duckdb.DuckDBPyConnection,
) -> None:
    """AC3: an always over an unscored pair reconciles at exit 0.

    S4.4 keeps assertion edges out of `match_scores`, so the guard — which reads
    only persisted scored rows — cannot see them; a NULL-`model_version` branch
    "fixing" a trip here would mean the persistence rule broke first.
    """
    unscored = int(
        scalar(
            reconciled,
            f"SELECT count(*) FROM {MATCH_SCORES} WHERE rec_a_key = ? AND rec_b_key = ?",
            *HOUSEHOLD_PAIR,
        )
    )
    assert unscored == 0, f"{HOUSEHOLD_PAIR} is scored; the AC3 probe needs an unscored pair"
    added = run_er(
        "assert",
        "add",
        "--a",
        HOUSEHOLD_PAIR[0],
        "--b",
        HOUSEHOLD_PAIR[1],
        "--kind",
        "always",
        "--by",
        "steward:er-085",
        "--note",
        "AC3: an assertion edge is never a scoring generation",
    )
    assert added.returncode == int(ExitCode.SUCCESS), added.stdout + added.stderr

    run_id = str(ULID())
    merged = run_er("reconcile", "--run-id", run_id)
    assert merged.returncode == int(ExitCode.SUCCESS), merged.stdout + merged.stderr
    entity_of = dict(membership_snapshot(reconciled))
    assert entity_of[HOUSEHOLD_PAIR[0]] == entity_of[HOUSEHOLD_PAIR[1]], (
        "the always did not merge its pair; the reconcile that was meant to prove "
        "the guard's indifference did nothing"
    )


def test_retrain_full_rescore_preserves_ids(
    reconciled: duckdb.DuckDBPyConnection, cfg: Config
) -> None:
    """AC4, AC5 / T-MODEL-1: activate v0002, rescore fully, ids survive."""
    before = membership_snapshot(reconciled)
    events_before = int(scalar(reconciled, f"SELECT count(*) FROM {EVENTS}"))

    # v0002 by registration (S12 M3: scenario tests never train): the committed
    # artifact under a new version, with its frozen TF copied under the same key.
    reconciled.execute(
        f"INSERT INTO {TF_LOOKUP} "
        f"SELECT ?, tf_snapshot_id, column_name, value, tf_value FROM {TF_LOOKUP} "
        "WHERE model_version = ?",
        [SECOND_VERSION, "v0001"],
    )
    reconciled.execute(f"UPDATE {MODEL_REGISTRY} SET status = 'superseded' WHERE status = 'active'")
    reconciled.execute(
        f"INSERT INTO {MODEL_REGISTRY} "
        f"SELECT ?, 'active', trained_at, corpus_snapshot, params_path, tf_tables_path, "
        f"tf_snapshot_id, config_hash, metrics, run_id FROM {MODEL_REGISTRY} "
        "WHERE model_version = ?",
        [SECOND_VERSION, "v0001"],
    )

    run_id = str(ULID())
    rescored = run_er("match", "--mode", "full", "--run-id", run_id, "--json")
    assert rescored.returncode == int(ExitCode.SUCCESS), rescored.stdout + rescored.stderr

    # AC4: after the full rescore the current edge set speaks one generation.
    generations = {
        (model, snapshot)
        for _a, _b, model, snapshot, probability in scoring_generation_rows(reconciled)
        if probability >= cfg.thresholds.review_low
    }
    assert generations == {(SECOND_VERSION, "01JWMDTFSNAP00000000000001")}, (
        f"current generations above review_low: {generations}"
    )

    settled = run_er("reconcile", "--run-id", run_id)
    assert settled.returncode in (
        int(ExitCode.SUCCESS),
        int(ExitCode.NOTHING_TO_DO),
    ), settled.stdout + settled.stderr

    # AC5: INV-PERM's set-equal clause across the version boundary — every id
    # survives, nothing was minted, nothing was announced.
    after = membership_snapshot(reconciled)
    assert after == before, "the rescore at an identical model moved a membership row"
    assert int(scalar(reconciled, f"SELECT count(*) FROM {EVENTS}")) == events_before


def test_splink_migration_requires_full_resolution_before_incrementals(reconciled) -> None:
    """Version refusal precedes invalidation; migration finishes only after assembly."""
    from er.versions import PINS

    before = reconciled.execute(f"SELECT * FROM {MATCH_SCORES} ORDER BY ALL").fetchall()
    reconciled.execute(
        f"UPDATE {MODEL_REGISTRY} SET metrics=json_merge_patch(metrics, "
        "'{\"splink_version\":\"4.0.16\"}') WHERE status='active'"
    )
    refused = run_er("match", "--mode", "full")
    assert refused.returncode == int(ExitCode.PRECONDITION), refused.stdout + refused.stderr
    assert "Train a new model" in refused.stderr
    assert reconciled.execute(f"SELECT * FROM {MATCH_SCORES} ORDER BY ALL").fetchall() == before
    # Registering/retraining the replacement supplies these two metadata fields.
    # The model stays fixed here so this tests lifecycle independently of quality.
    import json

    reconciled.execute(
        f"UPDATE {MODEL_REGISTRY} SET metrics=json_merge_patch(metrics, ?::JSON) "
        "WHERE status='active'",
        [
            json.dumps(
                {
                    "splink_version": PINS["splink"].version,
                    "migration_requires_full_resolution": True,
                }
            )
        ],
    )
    for stage in ("match", "reconcile", "assemble"):
        refused = run_er("match", "--mode", "incremental")
        assert refused.returncode == int(ExitCode.PRECONDITION), refused.stdout + refused.stderr
        assert "full matching, reconciliation and assembly" in refused.stderr
        args = ("--mode", "full") if stage == "match" else ()
        result = run_er(stage, *args, "--run-id", str(ULID()))
        assert result.returncode in (0, 10), result.stdout + result.stderr
    allowed = run_er("match", "--mode", "incremental")
    assert allowed.returncode in (0, 10), allowed.stdout + allowed.stderr
