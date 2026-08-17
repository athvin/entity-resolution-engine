"""`er match --mode incremental` over `incremental_batch` (S4.3.4, S4.3.5, S4.0, S4.0b).

Every claim here is about a real corpus scored by a real Splink model against a real
lake. The scenario is delivered in its two phases the way an operator would run them:
`base/` is ingested, standardized and scored by a **full** run, and only then is `batch/`
ingested, standardized and scored **incrementally**. Nothing is trained (S4.3.2 item 6)
and nothing is stubbed — the one monkeypatch in the file is the falsification of AC2 and
exists to make a pass *stop* contributing.

**Why the two-phase shape is the test and not just its setup.** S4.3.4's two passes are
distinguished by which records each one can pair, so a suite that scored everything at
once could not tell them apart. `fixtures/static/incremental_batch/attribution.csv`
names, per batch record, which pass reaches it: four records are `pass1` (three joiners
onto base personas and the `crm:C010` bridge that merges two base entities), and two —
`billing:B009` and `webforms:W010`, persona P12 — are `pass2`, because they are the same
person arriving in one batch and `find_matches_to_new_records` never pairs two new
records with each other. That last pair is the only thing standing between "two passes"
and "one pass plus a comment", which is why
:func:`test_pass2_removal_loses_the_new_pair` deletes pass 2 and asserts the pair goes
with it.

**What the base run has to leave behind for AC6 to be reachable.** `base_10`'s persona
P10 is one source record (`crm:C008`, entity E9) and pairs with nothing, so it is never
an endpoint of a `match_scores` row. The batch's `webforms:W009` is that same persona
arriving a second time — which is exactly why `unscored_record_keys` cannot mean "has no
scored row": C008 would be re-offered forever and S4.0's ``10`` would be unreachable. The
second clause of that helper (its `ingest_batch_id` contributed no scored endpoint) is
what this suite exercises without having to say so, in
:func:`test_match_scores_is_cumulative_and_idempotent`.

The `Dbt` harness and the delivery helper are duplicated from
`tests/integration/test_full_match.py` rather than imported, for the reason that module
states: a test module importing another test module makes a node id's dependencies
invisible to whoever reads it.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import fixture_settings, load_fixture_model
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config, Thresholds
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.ids import record_key
from er.errors import ExitCode
from er.lake.ducklake import LAKE_ALIAS, attach_statements, detach
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching import incremental
from er.matching.api import assert_no_splink_relations_in_lake, leaked_splink_relations
from er.matching.edges import current_edges
from er.matching.evidence import GAMMA_PREFIX
from er.matching.full import MATCH_SCORES_RELATION, score_full
from er.matching.incremental import (
    MODE_INCREMENTAL,
    IncrementalScoreResult,
    score_incremental,
    unscored_record_keys,
)
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.matching.thresholds import in_gray_band, is_auto_merge
from er.obs.counters import DECLARED_COUNTERS, StageCounters
from er.obs.runctx import StageRun
from er.review.queue import GRAY_BAND, OPEN, PAIR, REVIEW_QUEUE_RELATION

#: The S8.2 fixture this suite scores, and its two phases in delivery order.
SCENARIO_NAME: Final = "incremental_batch"
BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"

#: The two dbt selectors `er standardize` runs (S4.2).
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.{MATCH_SCORES_RELATION}"
REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.{REVIEW_QUEUE_RELATION}"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

#: S5's `match_scores` column list, in DDL order, read from the one place that declares
#: it — the same source the merge under test is generated from.
MATCH_SCORE_COLUMNS: Final[tuple[str, ...]] = REGISTRY[MATCH_SCORES_RELATION].column_names

#: The stage name S5 records this work under, for the `run_stages` row AC7 reads.
MATCH_STAGE: Final = "match"

#: The batch pair only pass 2 can produce: persona P12 arriving twice in one delivery
#: (`fixtures/static/incremental_batch/attribution.csv`, role `new_pair`). Canonical
#: order, because every pair relation carries `rec_a_key < rec_b_key` (S5.0).
NEW_PAIR: Final[tuple[str, str]] = ("billing:B009", "webforms:W010")

#: The bridge `incremental_batch/scenario.yaml` declares: `crm:C010` carries base persona
#: P6's phone, address and birth date and P7's email, so it joins two base entities. A
#: `pass1` record, and the reason pass 1's own contribution is not vacuous either.
BRIDGE_KEY: Final = "crm:C010"


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


def score_rows(connection: duckdb.DuckDBPyConnection) -> list[dict[str, Any]]:
    """Every `match_scores` row, keyed by S5's column names, in canonical order."""
    rows = connection.execute(
        f"SELECT {', '.join(MATCH_SCORE_COLUMNS)} FROM {MATCH_SCORES} ORDER BY rec_a_key, rec_b_key"
    ).fetchall()
    return [dict(zip(MATCH_SCORE_COLUMNS, row, strict=True)) for row in rows]


def pair_set(rows: list[dict[str, Any]]) -> set[tuple[str, str]]:
    """The canonical pairs ``rows`` holds."""
    return {(str(row["rec_a_key"]), str(row["rec_b_key"])) for row in rows}


def queued_pairs(connection: duckdb.DuckDBPyConnection) -> list[dict[str, Any]]:
    """Every open `gray_band` row of `review_queue`, in pair order."""
    columns = ("rec_a_key", "rec_b_key", "match_probability", "waterfall", "last_seen_run_id")
    rows = connection.execute(
        f"SELECT {', '.join(columns)} FROM {REVIEW_QUEUE} "
        f"WHERE subject_type = ? AND reason = ? AND status = ? ORDER BY rec_a_key, rec_b_key",
        [PAIR, GRAY_BAND, OPEN],
    ).fetchall()
    return [dict(zip(columns, row, strict=True)) for row in rows]


def content_hashes(connection: duckdb.DuckDBPyConnection) -> dict[str, str]:
    """`int_std_records`' current `record_key -> content_hash`, the INV-SCORE key half."""
    rows = connection.execute(f"SELECT record_key, content_hash FROM {STD_RECORDS}").fetchall()
    return {str(record_key): str(content_hash) for record_key, content_hash in rows}


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


def _manifest_line(stdout: str) -> dict[str, Any]:
    """The `--json` manifest `er match` wrote, picked out of everything else on stdout.

    Selected by its key set rather than by position: Splink and DuckDB both write
    progress lines, and S4.0's claim is that the command's own output is on stdout — not
    that it is the last thing there.
    """
    for line in stdout.splitlines():
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict) and "pairs_scored" in payload:
            return payload
    raise AssertionError(f"no `er match` manifest on stdout:\n{stdout}")


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
    """One completed in-process incremental pass, and the row it recorded into."""

    result: IncrementalScoreResult
    stage_run: StageRun


def score_batch(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    model: tuple[str, str],
    *,
    record_keys: tuple[str, ...] | None = None,
) -> Scored:
    """One `er match --mode incremental` stage, in process, on this test's connection."""
    stage_run = new_stage_run()
    model_version, tf_snapshot_id = model
    result = score_incremental(
        connection,
        cfg,
        stage_run,
        model_version=model_version,
        tf_snapshot_id=tf_snapshot_id,
        settings=fixture_settings(),
        record_keys=record_keys,
    )
    return Scored(result=result, stage_run=stage_run)


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return config()


@pytest.fixture
def scenario() -> Scenario:
    """The S8.2 fixture, opened through ER-028's loader."""
    loaded = load_scenario(SCENARIO_NAME)
    assert loaded.phases == (BASE_PHASE, BATCH_PHASE)
    return loaded


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


@dataclass
class Substrate:
    """The lake after the base full run, with the handles the batch phase needs."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    dbt: Dbt
    scenario: Scenario
    delivery_root: Path
    model: tuple[str, str]
    base_rows: list[dict[str, Any]]

    def ingest(self, phase: str) -> None:
        """Deliver and ingest one phase, then standardize, as `er run-all` would."""
        root = deliver(self.scenario, phase, self.delivery_root / phase)
        for source in self.scenario.inputs_for(phase):
            result = run_er("ingest", "--source", source, "--path", str(root))
            assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
        self.dbt.standardize()

    def batch_keys(self) -> tuple[str, ...]:
        """The `record_key`s of the `batch/` delivery, from the fixture's own CSVs.

        Derived from the fixture rather than from `unscored_record_keys`, so that AC1's
        "every persisted new row has at least one endpoint in `batch/`" is a claim about
        the delivery and not a restatement of whatever the implementation chose to score.
        """
        keys: list[str] = []
        for source, path in self.scenario.inputs_for(BATCH_PHASE).items():
            lines = path.read_text(encoding="utf-8").splitlines()
            # The first column of every S8.2 input CSV is that source's own record id
            # (`fixtures/static/FORMAT.md`); `record_key` is the one place the two are
            # joined into S5.0's scalar identity.
            keys += [record_key(source, line.split(",")[0]) for line in lines[1:] if line]
        return tuple(sorted(keys))


@pytest.fixture
def substrate(
    scenario: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[Substrate]:
    """`base/` ingested, standardized and scored by a full run; `batch/` not yet delivered.

    The teardown restores the connection's default schema. `splink_api` issues
    ``SET schema 'splink_scratch'`` on whatever connection it is handed, and this one is
    the session handle every other suite shares — leaving it repointed would make a
    later test's unqualified statement land somewhere it did not choose.
    """
    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    ready = Substrate(
        connection=initialised_lake,
        cfg=cfg,
        dbt=dbt,
        scenario=scenario,
        delivery_root=tmp_path / "drop",
        model=("", ""),
        base_rows=[],
    )
    dbt("seed")
    ready.ingest(BASE_PHASE)

    model_version, tf_snapshot_id, _ = load_fixture_model(initialised_lake)
    ready.model = (model_version, tf_snapshot_id)

    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        base_run = new_stage_run()
        score_full(
            initialised_lake,
            cfg,
            base_run,
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            settings=fixture_settings(),
        )
        ready.base_rows = score_rows(initialised_lake)
        assert ready.base_rows, "the base full run scored nothing; every claim below is vacuous"
        ready.ingest(BATCH_PHASE)
        yield ready
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def test_two_passes_are_unioned(substrate: Substrate) -> None:
    """AC1: the batch scores, and both passes are visibly in the result.

    The union is asserted from both sides. Pass 2's contribution is `NEW_PAIR`, whose two
    endpoints are both in `batch/` — a pair `find_matches_to_new_records` cannot produce.
    Pass 1's is the bridge, which pairs a batch record with base records — a pair the
    batch-only linker cannot produce, because neither of its partners is in the batch.
    A run missing either pass satisfies one of these and fails the other.
    """
    batch = set(substrate.batch_keys())
    scored = score_batch(substrate.connection, substrate.cfg, substrate.model)
    assert scored.result.exit_code == int(ExitCode.SUCCESS)
    assert scored.result.unscored_records == len(batch), (
        f"the stage scored {scored.result.unscored_records} record(s) and `batch/` delivers "
        f"{len(batch)}; the batch is exactly the delivery"
    )

    rows = score_rows(substrate.connection)
    before = pair_set(substrate.base_rows)
    added = pair_set(rows) - before
    assert added, "the incremental run added no pair at all"
    for rec_a_key, rec_b_key in added:
        assert batch & {rec_a_key, rec_b_key}, (
            f"({rec_a_key}, {rec_b_key}) has no endpoint in `batch/`; incremental scoring "
            f"pairs the batch against the corpus and against itself, and nothing else"
        )

    # Pass 2's half: both endpoints new.
    batch_internal = sorted(pair for pair in added if set(pair) <= batch)
    assert NEW_PAIR in added, (
        f"{NEW_PAIR} is absent; the two `new_pair` batch records form a new entity and only "
        f"the S4.3.4 batch-only `dedupe_only` pass can pair them. Batch-internal pairs the "
        f"run did produce: {batch_internal or 'none at all — pass 2 contributed nothing'}"
    )
    assert set(NEW_PAIR) <= batch

    # Pass 1's half: a batch record paired with a record that is NOT in the batch.
    bridge = {pair for pair in added if BRIDGE_KEY in pair and not set(pair) <= batch}
    assert bridge, (
        f"{BRIDGE_KEY} is paired with no base record; the declared bridge is a `pass1` "
        f"record and `find_matches_to_new_records` is the only pass that reaches the corpus"
    )


def test_pass2_removal_loses_the_new_pair(
    substrate: Substrate, monkeypatch: pytest.MonkeyPatch
) -> None:
    """AC2: delete pass 2 and the new-vs-new pair goes with it.

    The falsification the ticket asks for. Pass 2 is replaced by a function returning
    ``None`` — the module's spelling for "this pass contributed no rows", i.e. an empty
    frame — leaving pass 1 to run exactly as before against exactly the same batch. If
    `find_matches_to_new_records` could pair two new records with each other, this test
    would pass with `NEW_PAIR` still present and :func:`test_two_passes_are_unioned`
    would be asserting a tautology.
    """
    calls: list[str] = []

    def no_pass2(*args: Any, **kwargs: Any) -> None:
        calls.append("pass2")
        return None

    monkeypatch.setattr(incremental, "pass2_new_vs_new", no_pass2)
    crippled = score_batch(substrate.connection, substrate.cfg, substrate.model)
    assert crippled.result.exit_code == int(ExitCode.SUCCESS)

    assert calls == ["pass2"], "pass 2 was not called; the falsification proves nothing"
    added = pair_set(score_rows(substrate.connection)) - pair_set(substrate.base_rows)
    assert added, "pass 1 alone added nothing, so the absence below is not pass 2's doing"
    assert NEW_PAIR not in added, (
        f"{NEW_PAIR} survived the removal of pass 2, so pass 1 produced it: "
        f"`find_matches_to_new_records` is pairing new records with each other and pass 2 "
        f"is unfalsifiable (S4.3.4)"
    )
    # Pass 1's own contribution is intact, which is what makes the loss attributable.
    assert any(BRIDGE_KEY in pair for pair in added)


def test_persisted_rows_are_canonical_and_keyed(substrate: Substrate) -> None:
    """AC3: canonical, self-pair-free, one row per logical key, hashes current, active."""
    model_version, tf_snapshot_id = substrate.model
    score_batch(substrate.connection, substrate.cfg, substrate.model)
    rows = score_rows(substrate.connection)
    hashes = content_hashes(substrate.connection)
    assert rows

    for row in rows:
        assert row["rec_a_key"] != row["rec_b_key"], f"self-pair {row['rec_a_key']!r} persisted"
        assert row["rec_a_key"] < row["rec_b_key"], f"{row['rec_a_key']} !< {row['rec_b_key']}"
        assert row["is_active"] is True
        assert row["model_version"] == model_version
        assert row["tf_snapshot_id"] == tf_snapshot_id
        assert row["run_id"] and row["scored_at"] is not None
        # The two halves of the INV-SCORE key, and they are the endpoints' OWN current
        # hashes: a row carrying some other record's — or a superseded — hash would make
        # the invariant uncheckable in exactly the way S4.3.3 says storing them prevents.
        assert row["rec_a_content_hash"] == hashes[row["rec_a_key"]]
        assert row["rec_b_content_hash"] == hashes[row["rec_b_key"]]
        evidence = json.loads(str(row["evidence"]))
        assert any(key.startswith(GAMMA_PREFIX) for key in evidence), (
            f"({row['rec_a_key']}, {row['rec_b_key']}) carries no gamma_* vector; S4.3.5 "
            f"requires the comparison vector to be retained rather than projected away"
        )

    duplicates = scalar(
        substrate.connection,
        f"SELECT count(*) - count(DISTINCT (rec_a_key, rec_b_key, model_version, tf_snapshot_id)) "
        f"FROM {MATCH_SCORES}",
    )
    assert duplicates == 0, (
        f"{duplicates} rows share a logical key; S4.3.4 permits at most one per "
        f"(model_version, tf_snapshot_id, rec_a_key, rec_b_key) regardless of is_active"
    )


def test_gray_band_is_queued_not_clustered(substrate: Substrate) -> None:
    """AC4: nothing below `review_low` is persisted, and the band is queued, not an edge.

    The natural band over this corpus may well be empty — its duplicates score near
    ``1.0`` — so the assertion is made twice: once against the document's own thresholds,
    and once with `auto_merge` moved onto an *observed* probability, which puts real pairs
    on both sides of the upper edge. The lower edge is unreachable from here at all:
    `predict(threshold_match_probability=p)` filters strictly above ``p``.
    """
    model_version, tf_snapshot_id = substrate.model
    scored = score_batch(substrate.connection, substrate.cfg, substrate.model)
    rows = score_rows(substrate.connection)

    below = [row for row in rows if row["match_probability"] < substrate.cfg.thresholds.review_low]
    assert not below, f"{len(below)} rows fall below review_low; nothing under it is persisted"

    natural = {
        (row["rec_a_key"], row["rec_b_key"])
        for row in rows
        if in_gray_band(row["match_probability"], substrate.cfg.thresholds)
    }
    assert scored.result.pairs_in_gray_band == len(natural)

    # Moving `auto_merge` onto an observed probability is what gives the band pairs. The
    # crafted document is scored over the SAME batch, explicitly: the lake now calls
    # those records scored, and the claim is about the band, not about batch selection.
    observed = sorted({row["match_probability"] for row in rows})
    assert len(observed) > 1, "every pair scored identically; the band has no edge to test"
    edge = observed[-1]
    crafted = substrate.cfg.model_copy(
        update={
            "thresholds": Thresholds(
                auto_merge=edge, review_low=substrate.cfg.thresholds.review_low
            )
        }
    )
    batch = substrate.batch_keys()
    second = score_batch(substrate.connection, crafted, substrate.model, record_keys=batch)

    # Scoped to the pairs THIS run wrote. `match_scores` also holds the base full run's
    # rows, which were classified against the document's own `auto_merge` and are not
    # this run's to have queued — S4.3.5's upsert refreshes and inserts, and closes
    # nothing, so a stage is answerable for the pairs it scored and no others.
    touched = {
        (str(row["rec_a_key"]), str(row["rec_b_key"])): float(row["match_probability"])
        for row in score_rows(substrate.connection)
        if row["run_id"] == second.stage_run.run_id
    }
    assert touched, "the crafted re-score stamped no row"
    banded = {pair for pair, p in touched.items() if in_gray_band(p, crafted.thresholds)}
    at_edge = {pair for pair, p in touched.items() if p == edge}
    assert banded and at_edge, "the crafted band has nothing on one of its sides"
    assert second.result.pairs_in_gray_band == len(banded)

    queued = [
        row
        for row in queued_pairs(substrate.connection)
        if row["last_seen_run_id"] == second.stage_run.run_id
    ]
    for row in queued:
        assert in_gray_band(row["match_probability"], crafted.thresholds), (
            f"({row['rec_a_key']}, {row['rec_b_key']}) is queued at "
            f"{row['match_probability']}, outside [{crafted.thresholds.review_low}, {edge})"
        )
        # Every queued row carries the evidence a steward acts on (S4.3.5).
        waterfall = json.loads(str(row["waterfall"]))
        assert any(key.startswith(GAMMA_PREFIX) for key in waterfall)

    open_rows: dict[tuple[str, str], int] = {}
    for row in queued:
        key = (str(row["rec_a_key"]), str(row["rec_b_key"]))
        open_rows[key] = open_rows.get(key, 0) + 1
    assert set(open_rows) == banded, (
        "the open gray_band queue is not exactly the half-open band this run scored; "
        "S4.3.5 upserts the band and nothing else"
    )
    assert set(open_rows.values()) <= {1}, f"a pair holds two open rows: {open_rows}"

    # The upper edge is where the half-open band decides something: a pair at exactly
    # `auto_merge` is a match, is in the clustering edge set, and must NOT be queued.
    edges = {
        (rec_a_key, rec_b_key)
        for rec_a_key, rec_b_key, _ in current_edges(
            substrate.connection,
            model_version,
            tf_snapshot_id,
            min_probability=crafted.thresholds.auto_merge,
        )
    }
    assert at_edge <= edges, (
        f"{sorted(at_edge - edges)} score exactly auto_merge and are not in the edge set; "
        f"the clustering threshold IS auto_merge and the band is half-open (S4.3)"
    )
    assert not (edges & banded), (
        f"{sorted(edges & banded)} are both queued for review and above auto_merge; a "
        f"banded pair is not clustered (S4.3.5)"
    )
    assert all(is_auto_merge(p, crafted.thresholds) for pair, p in touched.items() if pair in edges)


def test_match_scores_is_cumulative_and_idempotent(substrate: Substrate) -> None:
    """AC5 and AC6: nothing is truncated, a re-score rewrites, and the next run exits 10."""
    model_version, tf_snapshot_id = substrate.model
    batch = substrate.batch_keys()

    first = score_batch(substrate.connection, substrate.cfg, substrate.model)
    after_first = score_rows(substrate.connection)

    # AC5: the base run's rows survived the incremental one, still active, and the
    # relation grew. `match_scores` is cumulative and is never truncated per run.
    assert len(after_first) > len(substrate.base_rows), (
        f"the relation went from {len(substrate.base_rows)} to {len(after_first)} rows; the "
        f"incremental run must add without removing (S4.3.4)"
    )
    surviving = {
        (row["rec_a_key"], row["rec_b_key"]): row
        for row in after_first
        if (row["rec_a_key"], row["rec_b_key"]) in pair_set(substrate.base_rows)
    }
    assert len(surviving) == len(substrate.base_rows), "the incremental run dropped a base row"
    for pair, row in surviving.items():
        assert row["is_active"] is True, f"{pair} was deactivated by a run that did not touch it"

    # AC6, first clause: the same batch again rewrites the same keys with equal numbers.
    second = score_batch(substrate.connection, substrate.cfg, substrate.model, record_keys=batch)
    after_second = score_rows(substrate.connection)

    assert len(after_second) == len(after_first), "the second pass over one batch added rows"
    assert first.result.pairs_scored == second.result.pairs_scored
    for one, two in zip(after_first, after_second, strict=True):
        assert (one["rec_a_key"], one["rec_b_key"]) == (two["rec_a_key"], two["rec_b_key"])
        # Bit-identical, not "close": INV-SCORE says `match_probability` is a pure
        # function of the key, and an approximate comparison would pass for an
        # implementation that recomputed term frequency from the corpus at hand.
        assert one["match_probability"] == two["match_probability"], (
            f"({one['rec_a_key']}, {one['rec_b_key']}) moved from "
            f"{one['match_probability']} to {two['match_probability']} between two "
            f"identical runs (INV-SCORE, S4.3.3)"
        )
    rewritten = {
        (row["rec_a_key"], row["rec_b_key"])
        for row in after_second
        if row["run_id"] == second.stage_run.run_id
    }
    assert rewritten, "the second pass stamped no row; the MERGE did not rewrite the keys"

    # AC6, second clause: with the batch scored, there is nothing left to score.
    remaining = unscored_record_keys(
        substrate.connection, model_version=model_version, tf_snapshot_id=tf_snapshot_id
    )
    assert remaining == (), (
        f"{list(remaining)} are still unscored after two passes over the batch, so S4.0's "
        f"`10` is unreachable"
    )
    third = score_batch(substrate.connection, substrate.cfg, substrate.model)
    assert third.result.exit_code == int(ExitCode.NOTHING_TO_DO)
    assert third.result.pairs_scored == 0
    assert len(score_rows(substrate.connection)) == len(after_second), (
        "the exit-10 run wrote to match_scores; a stage with nothing to do writes nothing"
    )


def test_counters_and_no_splink_relations(substrate: Substrate, object_store: ObjectStore) -> None:
    """AC7 and AC8: the CLI records S4.3.5's counters, exits 10 next, and leaks nothing.

    The real console script, twice: `er match --mode incremental` over the delivered
    batch, then again over a corpus it has now scored. Both invocations go through
    `model_registry.params_path` like any other run, which is why the fixture model's
    settings are published to the object store first.
    """
    model_version, tf_snapshot_id = substrate.model
    published = model_params_uri(substrate.cfg.storage.model_uri_prefix, model_version)
    object_store.put_bytes(published, json.dumps(fixture_settings()).encode("utf-8"))
    substrate.connection.execute(
        f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
        [published, model_version],
    )

    run_id = str(ULID())
    scored = run_er("match", "--mode", MODE_INCREMENTAL, "--run-id", run_id, "--json")
    assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr

    manifest = _manifest_line(scored.stdout)
    assert manifest["mode"] == MODE_INCREMENTAL
    assert manifest["model_version"] == model_version
    assert manifest["tf_snapshot_id"] == tf_snapshot_id
    assert manifest["pairs_scored"] > 0

    columns = (
        "candidate_pairs",
        "pairs_above_auto_merge",
        "review_queue_added",
        "counters",
        "rows_in",
        "rows_out",
    )
    found = substrate.connection.execute(
        f"SELECT {', '.join(columns)} FROM {RUN_STAGES} WHERE run_id = ? AND stage = ?",
        [run_id, MATCH_STAGE],
    ).fetchall()
    assert len(found) == 1, f"{len(found)} run_stages rows for {MATCH_STAGE!r}; S5.2 writes one"
    recorded = dict(zip(columns, found[0], strict=True))

    # The typed columns S5.2 promotes and this stage measures. `candidate_pairs` is the
    # one it does NOT: S4.3.4 assigns `candidate_pair_count` to `int_blocking_keys`,
    # which is not an input to incremental scoring, and S5.2 spells "the stage does not
    # report this" as NULL rather than as an absent column.
    assert recorded["candidate_pairs"] is None
    assert recorded["pairs_above_auto_merge"] is not None
    assert recorded["review_queue_added"] is not None
    assert recorded["rows_in"] == len(substrate.batch_keys())
    assert recorded["rows_out"] == manifest["pairs_scored"]

    counters = json.loads(str(recorded["counters"]))
    for name in (
        "mode",
        "model_version",
        "tf_snapshot_id",
        "pairs_scored",
        "pairs_in_gray_band",
        "review_queue_refreshed",
        "duration_ms",
    ):
        assert counters.get(name) is not None, f"counters JSON is missing {name} (S4.3.5)"
    assert "candidate_pairs" in counters, "a NULL counter is reported, not omitted (S5.2)"
    assert counters["mode"] == MODE_INCREMENTAL
    assert counters["pairs_scored"] == manifest["pairs_scored"]

    # AC8. The stage ran in its own process, so nothing of Splink's could survive in
    # memory; what the assertion is about is the lake, which outlives every process.
    assert leaked_splink_relations(substrate.connection) == ()
    assert_no_splink_relations_in_lake(substrate.connection)
    assert (
        scalar(
            substrate.connection,
            "SELECT count(*) FROM duckdb_tables() WHERE database_name = ? "
            "AND table_name LIKE '__splink__%'",
            LAKE_ALIAS,
        )
        == 0
    )

    # S4.0's `10`: the same command over a corpus with no unscored record left.
    again = run_er("match", "--mode", MODE_INCREMENTAL, "--json")
    assert again.returncode == int(ExitCode.NOTHING_TO_DO), again.stdout + again.stderr
    assert _manifest_line(again.stdout)["pairs_scored"] == 0
