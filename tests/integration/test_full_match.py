"""`er match --mode full` over `base_10` with the committed model (S4.3.4, S4.3.5, S4.0b).

Every claim in this file is about a real corpus scored by a real Splink model against a
real lake: `base_10` is delivered through `er ingest`, standardized through dbt, and
scored at the committed `model_test_v1` with its frozen `tf_lookup` rows. Nothing is
trained (S4.3.2 item 6: EM over 23 records is degenerate) and nothing is stubbed —
`score_full` is called with the connection the harness attached, which is what makes
"exactly one write statement reached `lake.main.match_scores`" a statement about the
engine rather than about a mock.

**How the two spies work, and why they are spies rather than inspections.**

* *Statements* are captured with DuckDB's own ``QueryLog``: logging is enabled around
  the stage and disabled after it, so `duckdb_logs` holds exactly the statements the
  stage issued on the lake connection — Splink's intermediates included. Counting
  writes to `match_scores` in that log is the S4.0b claim in its own terms. Reading the
  relation afterwards could not distinguish one `MERGE` from a hundred `INSERT`s.
* *The Splink threshold* is captured by wrapping `LinkerInference.predict`. S4.3's
  "never rely on Splink's ``-4`` default" is a claim about the argument the stage
  passes, and the only place that argument exists is the call.

**Two things the corpus cannot show, and where they are covered instead.** `base_10`'s
duplicate pairs score near 1.0, so the natural gray band may well be empty — a test
that only looked at it would assert nothing. The gray-band test therefore scores a
second time with `auto_merge` moved onto an *observed* probability,
which puts real pairs on both sides of the upper edge. The lower edge is unreachable
from here at all: Splink's `predict(threshold_match_probability=p)` filters strictly
above ``p``, so a pair at exactly `review_low` is never scored, and
``in_gray_band(review_low) is True`` is asserted in
`tests/unit/matching/test_thresholds.py` as AC5 says it is.

The `Dbt` harness and the delivery helper are duplicated from
`tests/integration/scenarios/test_blocking_keys.py` rather than imported, for the
reason that module states: a test module importing another test module makes a node
id's dependencies invisible to whoever reads it.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator, Mapping
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import fixture_settings, load_fixture_model
from helpers.scenario import Scenario, load_scenario
from splink.internals.linker_components.inference import LinkerInference
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config, Thresholds
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode
from er.lake.columns import VOLATILE_COLUMNS
from er.lake.ducklake import attach_statements, detach
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.api import assert_no_splink_relations_in_lake, leaked_splink_relations
from er.matching.evidence import (
    BAYES_FACTOR_PREFIX,
    GAMMA_PREFIX,
    MATCH_WEIGHT_KEY,
    TF_ADJUSTMENT_PREFIX,
)
from er.matching.full import MATCH_SCORES_RELATION, MODE_FULL, FullMatchResult, score_full
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.matching.thresholds import in_gray_band, is_auto_merge
from er.obs.counters import DECLARED_COUNTERS, StageCounters
from er.obs.runctx import StageRun
from er.review.queue import GRAY_BAND, OPEN, PAIR, REVIEW_QUEUE_RELATION

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

#: The S8.2 fixture this suite scores, and its only phase.
SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

#: The two dbt selectors `er standardize` runs (S4.2).
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.{MATCH_SCORES_RELATION}"
REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.{REVIEW_QUEUE_RELATION}"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

#: S5's `match_scores` column list, in DDL order, read from the one place that declares
#: it — the same source the merge under test is generated from.
MATCH_SCORE_COLUMNS: Final[tuple[str, ...]] = REGISTRY[MATCH_SCORES_RELATION].column_names

#: AC2's comparison set: everything except the run-scoped stamps, which are *expected*
#: to move between two runs that produced identical data (S5.0, M7).
STABLE_COLUMNS: Final[tuple[str, ...]] = tuple(
    column for column in MATCH_SCORE_COLUMNS if column not in VOLATILE_COLUMNS
)

#: The statement verbs that write. A `SELECT` naming `match_scores` is a read, and the
#: stage makes several — the point of AC3 is that exactly one statement *wrote*.
WRITE_VERBS: Final[frozenset[str]] = frozenset(
    {"INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "COPY", "DROP", "TRUNCATE", "ALTER"}
)

#: The stage name S5 records this work under, for the `run_stages` row AC7 reads.
MATCH_STAGE: Final = "match"


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


def score_rows(connection: duckdb.DuckDBPyConnection) -> list[dict[str, Any]]:
    """Every `match_scores` row, keyed by S5's column names, in canonical order."""
    rows = connection.execute(
        f"SELECT {', '.join(MATCH_SCORE_COLUMNS)} FROM {MATCH_SCORES} ORDER BY rec_a_key, rec_b_key"
    ).fetchall()
    return [dict(zip(MATCH_SCORE_COLUMNS, row, strict=True)) for row in rows]


def queued_pairs(connection: duckdb.DuckDBPyConnection) -> list[dict[str, Any]]:
    """Every open `gray_band` row of `review_queue`, in pair order."""
    columns = ("rec_a_key", "rec_b_key", "match_probability", "waterfall", "last_seen_run_id")
    rows = connection.execute(
        f"SELECT {', '.join(columns)} FROM {REVIEW_QUEUE} "
        f"WHERE subject_type = ? AND reason = ? AND status = ? ORDER BY rec_a_key, rec_b_key",
        [PAIR, GRAY_BAND, OPEN],
    ).fetchall()
    return [dict(zip(columns, row, strict=True)) for row in rows]


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
class StatementLog:
    """DuckDB's `QueryLog`, scoped to one block: the statement spy of AC3.

    Enabled on entry and disabled on exit, so `duckdb_logs` holds the statements
    issued *inside* the block and nothing else — logging is off everywhere else in the
    suite, and a statement executed while it is off is never recorded.
    """

    connection: duckdb.DuckDBPyConnection

    def __enter__(self) -> StatementLog:
        self.connection.execute("CALL enable_logging('QueryLog')")
        return self

    def __exit__(self, *exception: object) -> None:
        self.connection.execute("CALL disable_logging()")

    def statements(self) -> tuple[str, ...]:
        rows = self.connection.execute(
            "SELECT message FROM duckdb_logs WHERE type = 'QueryLog'"
        ).fetchall()
        return tuple(str(message) for (message,) in rows)

    def writes_to(self, relation: str) -> tuple[str, ...]:
        """Every logged statement that WROTE to ``relation``, in log order."""
        return tuple(
            statement
            for statement in self.statements()
            if relation in statement and _verb(statement) in WRITE_VERBS
        )


def _manifest_line(stdout: str) -> dict[str, Any]:
    """The `--json` manifest `er match` wrote, picked out of everything else on stdout.

    Selected by its key set rather than by position: Splink and DuckDB both write
    progress lines, and S4.0's claim is that the command's own output is on stdout —
    not that it is the last thing there.
    """
    for line in stdout.splitlines():
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict) and "pairs_scored" in payload:
            return payload
    raise AssertionError(f"no `er match` manifest on stdout:\n{stdout}")


def _verb(statement: str) -> str:
    """The leading keyword of ``statement``, upper-cased; ``''`` for an empty one."""
    tokens = statement.strip().split(maxsplit=1)
    return tokens[0].upper() if tokens else ""


@dataclass
class PredictSpy:
    """Every `threshold_match_probability` the stage passed Splink, in call order."""

    calls: list[Mapping[str, Any]] = field(default_factory=list)

    @property
    def thresholds(self) -> list[Any]:
        return [call.get("threshold_match_probability") for call in self.calls]


def new_stage_run(run_id: str | None = None) -> StageRun:
    """A `run_stages` row for one in-process stage, with S4.3.5's counter vocabulary."""
    return StageRun(
        run_id=run_id if run_id is not None else str(ULID()),
        stage=MATCH_STAGE,
        seq=1,
        started_at=datetime.now(UTC),
        counters=StageCounters(DECLARED_COUNTERS[MATCH_STAGE]),
    )


@dataclass(frozen=True)
class Scored:
    """One completed in-process scoring pass, and the row it recorded into."""

    result: FullMatchResult
    stage_run: StageRun


def score(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    model: tuple[str, str],
    *,
    run_id: str | None = None,
) -> Scored:
    """One `er match --mode full` stage, in process, on this test's connection."""
    stage_run = new_stage_run(run_id)
    model_version, tf_snapshot_id = model
    result = score_full(
        connection,
        cfg,
        stage_run,
        model_version=model_version,
        tf_snapshot_id=tf_snapshot_id,
        settings=fixture_settings(),
    )
    return Scored(result=result, stage_run=stage_run)


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


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored.

    The image ships no `dbt/dbt_packages` (`.dockerignore`) and the intermediate build
    runs `dbt_utils` tests. A plain subprocess: `deps` touches no warehouse.
    """
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
    """`base_10` ingested and standardized on this test's fresh namespace.

    The teardown restores the connection's default schema. `splink_api` issues
    ``SET schema 'splink_scratch'`` on whatever connection it is handed, and this one
    is the session handle every other suite shares — leaving it repointed would make a
    later test's unqualified statement land somewhere it did not choose.
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
        yield initialised_lake
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


@pytest.fixture
def model(standardized: duckdb.DuckDBPyConnection) -> tuple[str, str]:
    """The committed model installed into this lake: `(model_version, tf_snapshot_id)`."""
    model_version, tf_snapshot_id, _ = load_fixture_model(standardized)
    return model_version, tf_snapshot_id


@pytest.fixture
def predict_spy(monkeypatch: pytest.MonkeyPatch) -> PredictSpy:
    """Record every `linker.inference.predict` call the stage makes (AC6)."""
    spy = PredictSpy()
    original = LinkerInference.predict

    def recording(self: LinkerInference, *args: Any, **kwargs: Any) -> Any:
        spy.calls.append(dict(kwargs))
        return original(self, *args, **kwargs)

    monkeypatch.setattr(LinkerInference, "predict", recording)
    return spy


def test_rows_are_canonical_keyed_and_above_review_low(
    standardized: duckdb.DuckDBPyConnection, cfg: Config, model: tuple[str, str]
) -> None:
    """AC1: every persisted row is canonical, active, fully provenanced and in band."""
    scored = score(standardized, cfg, model)
    rows = score_rows(standardized)

    assert rows, "the stage persisted nothing; every assertion below would be vacuous"
    assert len(rows) == scored.result.pairs_scored

    hashes = dict(
        standardized.execute(
            f"SELECT record_key, content_hash FROM {SCHEMA_QUALIFIER}.int_std_records"
        ).fetchall()
    )
    model_version, tf_snapshot_id = model
    for row in rows:
        assert row["rec_a_key"] < row["rec_b_key"], f"{row['rec_a_key']} !< {row['rec_b_key']}"
        assert row["is_active"] is True
        assert row["model_version"] == model_version
        assert row["tf_snapshot_id"] == tf_snapshot_id
        assert row["run_id"] and row["scored_at"] is not None
        # The two halves of the INV-SCORE key, and they are the endpoints' OWN hashes:
        # a row carrying some other record's hash would make the invariant uncheckable
        # in exactly the way S4.3.3 says storing them prevents (S4.3.3).
        assert row["rec_a_content_hash"] == hashes[row["rec_a_key"]]
        assert row["rec_b_content_hash"] == hashes[row["rec_b_key"]]
        assert row["match_probability"] >= cfg.thresholds.review_low

    below = [row for row in rows if row["match_probability"] < cfg.thresholds.review_low]
    assert not below, f"{len(below)} rows fall below review_low; nothing under it is persisted"


def test_rescoring_is_idempotent_on_the_logical_key(
    standardized: duckdb.DuckDBPyConnection, cfg: Config, model: tuple[str, str]
) -> None:
    """AC2: a second pass at the same key rewrites the same rows with the same numbers."""
    first = score(standardized, cfg, model)
    before = score_rows(standardized)

    second = score(standardized, cfg, model)
    after = score_rows(standardized)

    assert first.result.pairs_scored == second.result.pairs_scored
    assert len(before) == len(after) and before, "the two passes disagree on the row count"

    duplicates = scalar(
        standardized,
        f"SELECT count(*) - count(DISTINCT (rec_a_key, rec_b_key, model_version, tf_snapshot_id)) "
        f"FROM {MATCH_SCORES}",
    )
    assert duplicates == 0, f"{duplicates} rows share a logical key; the MERGE appended"

    for one, two in zip(before, after, strict=True):
        # Bit-identical, not "close": INV-SCORE says `match_probability` is a pure
        # function of the key, and an approximate comparison would pass for an
        # implementation that recomputed term frequency from the corpus at hand.
        assert one["match_probability"] == two["match_probability"]
        for column in STABLE_COLUMNS:
            assert one[column] == two[column], f"{column} moved between two identical runs"
    # The volatile half is expected to move: the second pass stamped its own run.
    assert {row["run_id"] for row in after} == {second.stage_run.run_id}


def test_single_write_statement_to_match_scores(
    standardized: duckdb.DuckDBPyConnection,
    cfg: Config,
    model: tuple[str, str],
    predict_spy: PredictSpy,
) -> None:
    """AC3, and AC6's spy half: one write, and a threshold that came from `review_low`."""
    with StatementLog(standardized) as log:
        scored = score(standardized, cfg, model)

    writes = log.writes_to(MATCH_SCORES_RELATION)
    assert len(writes) == 1, (
        f"{len(writes)} statements wrote to {MATCH_SCORES_RELATION}; S4.0b permits one "
        f"per stage:\n" + "\n---\n".join(writes)
    )
    assert _verb(writes[0]) == "MERGE", f"the write is not the S4.3.4 MERGE: {writes[0][:120]}"
    assert scored.result.pairs_scored > 0, "a stage that wrote nothing writes one statement too"

    # AC6: the threshold Splink received is `review_low` itself, in probabilities —
    # not Splink's -4 default, and not a weight passed into a probability parameter.
    assert predict_spy.thresholds == [cfg.thresholds.review_low], (
        f"predict() was called with {predict_spy.thresholds}, not [{cfg.thresholds.review_low}]"
    )


def test_evidence_payload_covers_every_comparison(
    standardized: duckdb.DuckDBPyConnection, cfg: Config, model: tuple[str, str]
) -> None:
    """AC4: one gamma and one Bayes factor per configured comparison, plus the weight."""
    score(standardized, cfg, model)
    rows = score_rows(standardized)
    assert rows

    configured = set(cfg.comparisons)
    tf_columns = {column for column, spec in cfg.comparisons.items() if spec.tf}
    for row in rows:
        evidence = json.loads(str(row["evidence"]))
        gammas = {
            key.removeprefix(GAMMA_PREFIX) for key in evidence if key.startswith(GAMMA_PREFIX)
        }
        assert gammas == configured, (
            f"evidence carries gamma_* for {sorted(gammas)}; the config declares "
            f"{sorted(configured)}"
        )
        for column in configured:
            assert f"{BAYES_FACTOR_PREFIX}{column}" in evidence
        for column in tf_columns:
            # `tf: true` means the score carries a frozen term-frequency adjustment
            # (D4); retaining it is what makes the number re-derivable.
            assert f"{TF_ADJUSTMENT_PREFIX}{column}" in evidence
        assert MATCH_WEIGHT_KEY in evidence


def test_gray_band_is_half_open_and_lands_in_review_queue(
    standardized: duckdb.DuckDBPyConnection, cfg: Config, model: tuple[str, str]
) -> None:
    """AC5: the queue is exactly the band, and `p == auto_merge` is outside it."""
    natural = score(standardized, cfg, model)
    rows = score_rows(standardized)
    queued = queued_pairs(standardized)

    # The whole queue, against the document's own thresholds: nothing at or above
    # `auto_merge`, and nothing below `review_low`.
    for row in queued:
        assert in_gray_band(row["match_probability"], cfg.thresholds)
    banded = {
        (row["rec_a_key"], row["rec_b_key"])
        for row in rows
        if in_gray_band(row["match_probability"], cfg.thresholds)
    }
    assert {(row["rec_a_key"], row["rec_b_key"]) for row in queued} == banded
    assert natural.result.pairs_in_gray_band == len(banded)

    # `base_10`'s duplicates score near 1.0, so the natural band may be empty. Moving
    # `auto_merge` onto an observed probability puts real pairs on both sides of the
    # upper edge, which is the edge that decides whether a steward is asked about a
    # pair clustering has already merged.
    observed = sorted({row["match_probability"] for row in rows})
    assert len(observed) > 1, "every pair scored identically; the band has no edge to test"
    edge = observed[-1]
    crafted = cfg.model_copy(
        update={"thresholds": Thresholds(auto_merge=edge, review_low=cfg.thresholds.review_low)}
    )

    second = score(standardized, crafted, model)
    below_edge = {
        (row["rec_a_key"], row["rec_b_key"])
        for row in score_rows(standardized)
        if row["match_probability"] < edge
    }
    at_edge = {
        (row["rec_a_key"], row["rec_b_key"])
        for row in score_rows(standardized)
        if row["match_probability"] == edge
    }
    assert below_edge and at_edge, "the crafted band has nothing on one of its sides"

    refreshed = {
        (row["rec_a_key"], row["rec_b_key"])
        for row in queued_pairs(standardized)
        if row["last_seen_run_id"] == second.stage_run.run_id
    }
    assert refreshed == below_edge, "the queue does not match the half-open band"
    assert not (refreshed & at_edge), (
        "a pair at exactly auto_merge was queued for review; the band is half-open "
        "and p == auto_merge is a match (S4.3)"
    )
    assert second.result.pairs_in_gray_band == len(below_edge)
    assert second.result.pairs_above_auto_merge == len(at_edge)
    assert is_auto_merge(edge, crafted.thresholds), "p == auto_merge is a match, not a review"

    # Every queued row carries the evidence a steward acts on (S4.3.5).
    for row in queued_pairs(standardized):
        waterfall = json.loads(str(row["waterfall"]))
        assert any(key.startswith(GAMMA_PREFIX) for key in waterfall)
        assert any(key.startswith(BAYES_FACTOR_PREFIX) for key in waterfall)


def test_counters_and_no_active_model_exit_3(
    standardized: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
) -> None:
    """AC7: the CLI exits 3 with no model, and writes S4.3.5's counters when it scores.

    Both arms are the real console script. The first runs against a lake whose
    `model_registry` is empty, which is S4.0's "3 no active model"; the second publishes
    the committed model's settings to the object store — `er match` loads them through
    `model_registry.params_path` like any other run — and then scores.
    """
    refused = run_er("match", "--mode", MODE_FULL)
    assert refused.returncode == int(ExitCode.PRECONDITION), refused.stdout + refused.stderr
    assert scalar(standardized, f"SELECT count(*) FROM {MATCH_SCORES}") == 0

    model_version, tf_snapshot_id, settings = load_fixture_model(standardized)
    published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
    object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
    standardized.execute(
        f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
        [published, model_version],
    )

    # An explicit `run_id`, because the refused invocation above also wrote a
    # `run_stages` row for this stage (S5.2 records a failure as faithfully as a
    # success) and the counters asserted below belong to this run alone.
    run_id = str(ULID())
    scored = run_er("match", "--mode", MODE_FULL, "--run-id", run_id, "--json")
    assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr

    manifest = _manifest_line(scored.stdout)
    assert manifest["mode"] == MODE_FULL
    assert manifest["model_version"] == model_version
    assert manifest["tf_snapshot_id"] == tf_snapshot_id
    assert manifest["pairs_scored"] > 0

    columns = ("candidate_pairs", "pairs_above_auto_merge", "counters", "rows_in", "rows_out")
    row = standardized.execute(
        f"SELECT {', '.join(columns)} FROM {RUN_STAGES} WHERE run_id = ? AND stage = ?",
        [run_id, MATCH_STAGE],
    ).fetchall()
    assert len(row) == 1, f"{len(row)} run_stages rows for {MATCH_STAGE!r}; S5.2 writes one"
    recorded = dict(zip(columns, row[0], strict=True))

    # The two typed columns S5.2 promotes and this stage measures.
    assert recorded["candidate_pairs"] is not None and recorded["candidate_pairs"] > 0
    assert recorded["pairs_above_auto_merge"] is not None
    assert recorded["rows_in"] > 0 and recorded["rows_out"] == manifest["pairs_scored"]

    counters = json.loads(str(recorded["counters"]))
    for name in (
        "mode",
        "model_version",
        "tf_snapshot_id",
        "pairs_scored",
        "pairs_in_gray_band",
        "review_queue_added",
        "duration_ms",
    ):
        assert counters.get(name) is not None, f"counters JSON is missing {name} (S4.3.5)"
    assert counters["mode"] == MODE_FULL
    assert counters["pairs_scored"] == manifest["pairs_scored"]


def test_no_splink_relations_in_lake(
    standardized: duckdb.DuckDBPyConnection, cfg: Config, model: tuple[str, str]
) -> None:
    """AC8: the stage scored a real corpus and left nothing of Splink's in the lake."""
    scored = score(standardized, cfg, model)
    assert scored.result.pairs_scored > 0, "a stage that scored nothing leaks nothing either"

    materialized = scalar(
        standardized,
        "SELECT count(*) FROM duckdb_tables() WHERE database_name = current_database() "
        "AND table_name LIKE '__splink__%'",
    )
    assert materialized > 0, "Splink materialized nothing; the leak check would be vacuous"

    assert leaked_splink_relations(standardized) == ()
    assert_no_splink_relations_in_lake(standardized)
