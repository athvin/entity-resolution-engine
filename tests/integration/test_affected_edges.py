"""S4.5.1's affected EDGE set against a real lake: the query, and what it excludes.

The adjustment's algebra is `tests/unit/entities/test_affected_edges.py`'s. Every claim
here is about a *relation*, and none of them is expressible in a synthetic edge list:

* **`match_scores` is cumulative** (S4.3.4). The misreading S4.5.1 names — loading only
  this run's scored pairs — is only visible against a table holding rows a previous run
  wrote, so :func:`test_prior_run_edges_are_loaded` writes them under a run id that is
  not the one under test and asserts the table holds nothing from the current run at all.
  An implementation that filtered on `run_id` would return an empty set there and
  fragment the entity into singletons.
* **`cut_edges` is a second relation with its own lifecycle** (S4.4.2). The exclusion and
  its release are two states of one row, and a test over a list could only assert that a
  set difference works.
* **`int_std_records` is dbt-owned**, and a tombstoned record's absence from it is
  produced by `er ingest --full-refresh-keys` followed by `er standardize` — not by
  leaving a key out of a fixture. S4.2 excludes the record from the standardized corpus
  while its `match_scores` row stays `is_active`, and that gap is exactly what AC4 is
  about.

**Why every test standardizes.** The edge set restricts both endpoints to
`int_std_records`, so the relation has to exist before any of this is answerable — and
`clean_lake` drops the dbt-owned relations between tests (S8.1), so it is built per test
rather than once. `base_10` is the smallest committed corpus that produces one.

**The edges are inserted, not scored.** Nothing here runs Splink: what is under test is
which of the rows in `match_scores` the loader returns, and scoring a corpus to obtain
them would make the probabilities a property of the frozen model rather than of the test.
The `(model_version, tf_snapshot_id)` pair is likewise a literal, exactly as
`tests/integration/test_current_edges.py` and `tests/integration/test_affected_nodes.py`
use one.

The `Dbt` harness and the delivery helpers are duplicated from
`tests/integration/test_affected_nodes.py` rather than imported, for the reason that
module states: a test module importing another test module makes a node id's
dependencies invisible to whoever reads it.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from datetime import datetime, timedelta
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
    ASSERTION_EDGE_PROBABILITY,
    ASSERTION_EVIDENCE_SOURCE,
    Edge,
    adjust_edges_with_assertions,
    affected_edges,
)
from er.entities.ids import record_key
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.review.assertions import ALWAYS, NEVER, active_assertions, add_assertion, retract_assertion

#: The S8.2 fixture this suite standardizes, and its only phase.
SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

#: The two dbt selectors `er standardize` runs (S4.2).
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"
CUT_EDGES: Final = f"{SCHEMA_QUALIFIER}.cut_edges"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"

#: The scoring key every `match_scores` row here carries, and the one the loader is
#: asked for. Literal because nothing in this suite scores: what is under test is which
#: rows come back, not what a model would have produced.
MODEL_VERSION: Final = "v0001"
TF_SNAPSHOT_ID: Final = "01JWMDTFSNAP00000000000070"

#: A second scoring key, for the rows AC2 requires be absent.
OTHER_MODEL_VERSION: Final = "v0002"
OTHER_TF_SNAPSHOT_ID: Final = "01JWMDTFSNAP00000000000071"

#: `thresholds.auto_merge` for this suite, and a probability inside the half-open gray
#: band below it (S4.3). The corpus is never scored, so both are the test's to choose.
AUTO_MERGE: Final = 0.90
GRAY_BAND_PROBABILITY: Final = 0.85
SCORED_PROBABILITY: Final = 0.99

#: Stamps for the rows written directly. Fixed so the ordering in a failure message is
#: readable rather than inferred from wall-clock ties.
SCORED_AT: Final = datetime(2026, 3, 1, 0, 0, 0)
CUT_AT: Final = SCORED_AT + timedelta(hours=1)

#: Five records of the `base_10` corpus, through S5.0's key helper rather than as
#: `"crm:C001"` literals — D6's scalar identity has one implementation and these rows
#: are joined against `int_std_records`, which is built by it.
CRM_1: Final = record_key("crm", "C001")
CRM_2: Final = record_key("crm", "C002")
CRM_8: Final = record_key("crm", "C008")
BILLING_1: Final = record_key("billing", "B001")
BILLING_2: Final = record_key("billing", "B002")

#: The affected node set most tests ask about. `BILLING_2` is in the corpus and NOT in
#: it, which is what makes "among its members" a checkable restriction rather than a
#: description of the whole edge table.
NODES: Final[frozenset[str]] = frozenset({BILLING_1, CRM_1, CRM_2})

STEWARD: Final = "integration-test"


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


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def insert_row(
    connection: duckdb.DuckDBPyConnection, relation: str, values: Mapping[str, Any]
) -> None:
    """One row of a `ddl.py`-owned relation, projected through S5's own column list.

    Positionally against :data:`~er.lake.model.REGISTRY` rather than against a literal
    tuple, so a column added to S5 leaves this helper writing NULL into it instead of
    failing on an arity nobody edited. Every name supplied must be a real column: a typo
    would otherwise be silently dropped and the row would be wrong in a way no assertion
    below could see.
    """
    columns = REGISTRY[relation].column_names
    unknown = sorted(set(values) - set(columns))
    assert not unknown, f"{relation} has no column(s) {unknown}; S5 declares {columns}"
    connection.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.{relation} ({', '.join(columns)}) "
        f"VALUES ({', '.join('?' for _ in columns)})",
        [values.get(column) for column in columns],
    )


def insert_edge(
    connection: duckdb.DuckDBPyConnection,
    pair: tuple[str, str],
    match_probability: float,
    *,
    run_id: str,
    is_active: bool = True,
    model_version: str = MODEL_VERSION,
    tf_snapshot_id: str = TF_SNAPSHOT_ID,
) -> None:
    """One `match_scores` row (S5), canonical and at a stated scoring key."""
    rec_a_key, rec_b_key = pair
    assert rec_a_key < rec_b_key, f"({rec_a_key!r}, {rec_b_key!r}) is not canonical (S5.0)"
    insert_row(
        connection,
        "match_scores",
        {
            "rec_a_key": rec_a_key,
            "rec_b_key": rec_b_key,
            "match_probability": match_probability,
            "model_version": model_version,
            "tf_snapshot_id": tf_snapshot_id,
            "rec_a_content_hash": f"{rec_a_key}-hash",
            "rec_b_content_hash": f"{rec_b_key}-hash",
            "evidence": "{}",
            "is_active": is_active,
            "run_id": run_id,
            "scored_at": SCORED_AT,
        },
    )


def insert_cut(
    connection: duckdb.DuckDBPyConnection,
    pair: tuple[str, str],
    *,
    assertion_id: str,
    run_id: str,
    model_version: str | None = None,
) -> str:
    """One active `cut_edges` row (S5), as S4.4.2's cut would have left it.

    Written directly because the writer is ER-076's: this ticket only EXCLUDES rows that
    are already there.
    """
    rec_a_key, rec_b_key = pair
    cut_id = str(ULID())
    insert_row(
        connection,
        "cut_edges",
        {
            "cut_id": cut_id,
            "rec_a_key": rec_a_key,
            "rec_b_key": rec_b_key,
            "match_probability": SCORED_PROBABILITY,
            "model_version": model_version,
            "assertion_id": assertion_id,
            "active": True,
            "cut_run_id": run_id,
            "cut_at": CUT_AT,
        },
    )
    return cut_id


def release_cut(connection: duckdb.DuckDBPyConnection, cut_id: str, *, run_id: str) -> None:
    """Deactivate one cut, as the retraction path will (S4.4.2, ER-076's to wire)."""
    connection.execute(
        f"UPDATE {CUT_EDGES} SET active = false, released_run_id = ?, released_at = ? "
        f"WHERE cut_id = ?",
        [run_id, CUT_AT, cut_id],
    )


def loaded(connection: duckdb.DuckDBPyConnection, nodes: frozenset[str]) -> list[Edge]:
    """The affected edge set for one node set, at this suite's scoring key."""
    return affected_edges(
        connection,
        nodes,
        model_version=MODEL_VERSION,
        tf_snapshot_id=TF_SNAPSHOT_ID,
        auto_merge=AUTO_MERGE,
    )


def pairs(edges: list[Edge]) -> list[tuple[str, str]]:
    """The pairs of an edge list, in the order it came back in."""
    return [edge.pair for edge in edges]


def standardized_keys(connection: duckdb.DuckDBPyConnection) -> set[str]:
    """Every `record_key` the standardized corpus currently holds (S4.2)."""
    rows = connection.execute(f"SELECT record_key FROM {STD_RECORDS}").fetchall()
    return {str(key) for (key,) in rows}


@dataclass(frozen=True)
class ScoreTable:
    """The two properties of `match_scores` AC6 is stated over."""

    rows: int
    unscored: int


def score_table(connection: duckdb.DuckDBPyConnection) -> ScoreTable:
    """`match_scores`' row count, and how many rows lack a scoring key.

    Both, because S4.4's guarantee has two halves: the adjustment adds no row, and every
    row that IS there is a scored one — `model_version` and `tf_snapshot_id` are `NOT
    NULL` (S5), so "no consumer needs a NULL-`model_version` branch".
    """
    return ScoreTable(
        rows=int(scalar(connection, f"SELECT count(*) FROM {MATCH_SCORES}")),
        unscored=int(
            scalar(
                connection,
                f"SELECT count(*) FROM {MATCH_SCORES} "
                f"WHERE model_version IS NULL OR tf_snapshot_id IS NULL",
            )
        ),
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


@dataclass(frozen=True)
class Corpus:
    """`base_10` standardized on this test's namespace, plus what a re-delivery needs."""

    connection: duckdb.DuckDBPyConnection
    scenario: Scenario
    dbt: Dbt
    root: Path


@pytest.fixture
def corpus(
    scenario: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[Corpus]:
    """`base_10` ingested and standardized, so `int_std_records` holds the endpoints.

    Every test needs it: S4.5.1 restricts both endpoints of every edge to the
    standardized corpus, so a suite that skipped this would be asserting against a
    relation that does not exist. Function-scoped because `clean_lake` drops the
    dbt-owned relations between tests (S8.1) — this is the cost of the restriction being
    real rather than mocked.
    """
    root = tmp_path / "drop"
    for source, path in scenario.inputs_for(PHASE).items():
        deliver({path.name: path.read_text(encoding="utf-8")}, root, source)
        ingest(source, root)

    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")
    dbt.standardize()
    assert {CRM_1, CRM_2, CRM_8, BILLING_1, BILLING_2} <= standardized_keys(initialised_lake)

    yield Corpus(connection=initialised_lake, scenario=scenario, dbt=dbt, root=root)


def test_prior_run_edges_are_loaded(corpus: Corpus) -> None:
    """AC1: edges written by a PREVIOUS run are the edge set of a later run.

    The ticket's reason for existing. `match_scores` is cumulative and is never truncated
    per run (S4.3.4), so an entity that was clustered weeks ago carries no row from
    today's run — and S4.5.1 says outright that an implementer who loads only this run's
    scored pairs "will spuriously fragment every touched entity".

    Constructed so that the misreading fails: every row is written under `prior_run`, the
    current run's id appears on no row at all, and the assertion below says so. An
    implementation that filtered by `run_id` would return the empty list here, and the
    three affected records would land in three singleton entities.

    The `(BILLING_2, CRM_8)` row is the other half of "among its members": it is a
    perfectly good active edge above the cut, and it is excluded because neither endpoint
    is an affected node.
    """
    prior_run = str(ULID())
    this_run = str(ULID())
    insert_edge(corpus.connection, (CRM_1, CRM_2), SCORED_PROBABILITY, run_id=prior_run)
    insert_edge(corpus.connection, (BILLING_1, CRM_1), 0.95, run_id=prior_run)
    insert_edge(corpus.connection, (BILLING_2, CRM_8), SCORED_PROBABILITY, run_id=prior_run)

    assert (
        scalar(corpus.connection, f"SELECT count(*) FROM {MATCH_SCORES} WHERE run_id = ?", this_run)
        == 0
    ), "the current run scored nothing; that is the whole construction of this test"

    edges = loaded(corpus.connection, NODES)

    assert pairs(edges) == [(BILLING_1, CRM_1), (CRM_1, CRM_2)]
    assert edges, "the affected entity's edge set is empty; it would fragment into singletons"
    assert all(edge.rec_a_key < edge.rec_b_key for edge in edges), "S5.0's canonical ordering"
    assert len({edge.pair for edge in edges}) == len(edges), "one row per canonical pair (S4.3.4)"
    assert all(edge.evidence is None for edge in edges), (
        "a scored edge carries no assertion payload"
    )


def test_only_active_edges_at_the_run_key_and_above_auto_merge(corpus: Corpus) -> None:
    """AC2: below the cut, invalidated, or at another key — all absent.

    Four exclusions in one test because they are four predicates of one query and each
    one of them, dropped, still returns a plausible-looking edge set. The banded pair is
    queued for review and "is **not** clustered" (S4.3.5); the invalidated row is what
    S4.5.5 leaves behind and must never be clustered again; and a row at another
    `model_version` or `tf_snapshot_id` is a different scoring problem (INV-SCORE,
    S4.3.3).

    The surviving pair sits at exactly `auto_merge`, which pins the bound as inclusive:
    the band is half-open (S4.3), so a pair at the cut is a match.
    """
    run_id = str(ULID())
    insert_edge(corpus.connection, (CRM_1, CRM_2), AUTO_MERGE, run_id=run_id)
    insert_edge(corpus.connection, (BILLING_1, CRM_1), GRAY_BAND_PROBABILITY, run_id=run_id)
    insert_edge(
        corpus.connection, (BILLING_1, CRM_2), SCORED_PROBABILITY, run_id=run_id, is_active=False
    )
    insert_edge(
        corpus.connection,
        (BILLING_2, CRM_1),
        SCORED_PROBABILITY,
        run_id=run_id,
        model_version=OTHER_MODEL_VERSION,
    )
    insert_edge(
        corpus.connection,
        (BILLING_2, CRM_2),
        SCORED_PROBABILITY,
        run_id=run_id,
        tf_snapshot_id=OTHER_TF_SNAPSHOT_ID,
    )

    wide = NODES | {BILLING_2}
    edges = loaded(corpus.connection, wide)

    assert pairs(edges) == [(CRM_1, CRM_2)]
    assert edges[0].match_probability == AUTO_MERGE, (
        "a pair at exactly auto_merge is a match and the band is half-open (S4.3)"
    )
    # And the rows that were excluded really are in the table: the emptiness above is the
    # predicate list's doing, not a failed insert.
    assert scalar(corpus.connection, f"SELECT count(*) FROM {MATCH_SCORES}") == 5


def test_cut_edges_are_excluded_and_released(corpus: Corpus) -> None:
    """AC3: an active cut removes the pair; deactivating the row brings it back.

    S4.4.2 states the exclusion as applying "on every subsequent run" and says what
    happens without it: every cut is silently re-merged from the cumulative table and
    `never` becomes a no-op with a one-run half-life. The release path is the same
    sentence read backwards — a cut is invalidated when its assertion is retracted, and
    the pair must then be clusterable again or the retraction would be inert.

    The `cut_edges` row is written at a DIFFERENT `model_version` than the run asks for.
    S5 makes that column nullable and S4.5.1's exclusion does not mention it: a cut is a
    statement about a pair, and a reader that scoped it to the run's key would re-merge
    every cut the first time a new model was trained.
    """
    run_id = str(ULID())
    insert_edge(corpus.connection, (CRM_1, CRM_2), SCORED_PROBABILITY, run_id=run_id)
    assert pairs(loaded(corpus.connection, NODES)) == [(CRM_1, CRM_2)]

    never = add_assertion(
        corpus.connection, a=CRM_1, b=CRM_2, kind=NEVER, created_by=STEWARD, created_at=SCORED_AT
    )
    cut_id = insert_cut(
        corpus.connection,
        (CRM_1, CRM_2),
        assertion_id=never.assertion_id,
        run_id=run_id,
        model_version=OTHER_MODEL_VERSION,
    )

    assert loaded(corpus.connection, NODES) == [], (
        "an active cut_edges row did not exclude its pair; without the exclusion the cut "
        "is re-merged on this very run (S4.4.2)"
    )

    retract_assertion(corpus.connection, never.assertion_id, retracted_by=STEWARD)
    release_cut(corpus.connection, cut_id, run_id=str(ULID()))

    assert pairs(loaded(corpus.connection, NODES)) == [(CRM_1, CRM_2)], (
        "the released cut still excludes its pair; a retraction that cannot restore the "
        "edge is a retraction that does nothing (S4.4.2)"
    )
    assert scalar(corpus.connection, f"SELECT count(*) FROM {CUT_EDGES}") == 1, (
        "cut_edges rows are deactivated, never deleted"
    )


def test_tombstoned_endpoint_excluded(corpus: Corpus) -> None:
    """AC4: an edge whose endpoint left `int_std_records` is excluded while still active.

    The gap S4.5.1's last two predicates exist for. `er ingest --full-refresh-keys`
    treats the delivery as the complete key set for `crm` (S4.1.1), so the omitted key
    gains a tombstone version and S4.2 drops it from the standardized corpus — but its
    `match_scores` rows are untouched: invalidation on deletion is S4.5.5's, and this
    ticket only honours the flag it finds. So the row is still `is_active=true` and the
    edge must still be gone, which is asserted in that order below.

    The surviving edge between two live records is the control: what was excluded is the
    tombstone's edge, not everything.
    """
    run_id = str(ULID())
    nodes = frozenset({CRM_1, CRM_2, CRM_8})
    insert_edge(corpus.connection, (CRM_1, CRM_2), SCORED_PROBABILITY, run_id=run_id)
    insert_edge(corpus.connection, (CRM_2, CRM_8), SCORED_PROBABILITY, run_id=run_id)
    assert pairs(loaded(corpus.connection, nodes)) == [(CRM_1, CRM_2), (CRM_2, CRM_8)]

    crm = corpus.scenario.inputs_for(PHASE)["crm"]
    header, *rows = crm.read_text(encoding="utf-8").splitlines()
    kept = [line for line in rows if line and record_key("crm", line.split(",")[0]) != CRM_8]
    assert len(kept) == len([line for line in rows if line]) - 1, "the trim removed the wrong count"
    deliver({crm.name: "\n".join([header, *kept]) + "\n"}, corpus.root.parent / "refresh", "crm")
    ingest("crm", corpus.root.parent / "refresh", "--full-refresh-keys")
    corpus.dbt.standardize()

    standardized = standardized_keys(corpus.connection)
    assert CRM_8 not in standardized, (
        f"{CRM_8} is still in int_std_records; S4.2 excludes a tombstoned record from the "
        f"standardized corpus, and this predicate exists because of that exclusion"
    )
    assert {CRM_1, CRM_2} <= standardized, "the refresh removed more than the omitted key"
    assert (
        scalar(
            corpus.connection,
            f"SELECT count(*) FROM {MATCH_SCORES} WHERE rec_b_key = ? AND is_active",
            CRM_8,
        )
        == 1
    ), "the tombstone's edge is still active; the exclusion below must not depend on that"

    assert pairs(loaded(corpus.connection, nodes)) == [(CRM_1, CRM_2)]


def test_assertion_edges_are_not_persisted(corpus: Corpus) -> None:
    """AC5 and AC6: the adjustment injects and removes in memory, and writes nothing.

    S4.4 is explicit on both halves — an active `always` "inserts an edge with
    `match_probability = 1.0` and `evidence = {"source":"assertion","assertion_id":…}`",
    an active `never` "removes the edge between the endpoints regardless of score", and
    **assertion edges are never persisted to `match_scores`**. The consequence the spec
    draws from that is the one asserted here: every `match_scores` row is a scored one,
    with `model_version` and `tf_snapshot_id` `NOT NULL`, so no consumer needs a
    NULL-`model_version` branch.

    The `always` pair was never scored — no `match_scores` row exists for it, which the
    count below pins — so the injected edge cannot be a row that was already there.
    """
    run_id = str(ULID())
    insert_edge(corpus.connection, (CRM_1, CRM_2), SCORED_PROBABILITY, run_id=run_id)
    always = add_assertion(
        corpus.connection,
        a=BILLING_1,
        b=CRM_1,
        kind=ALWAYS,
        created_by=STEWARD,
        created_at=SCORED_AT,
    )
    never = add_assertion(
        corpus.connection, a=CRM_1, b=CRM_2, kind=NEVER, created_by=STEWARD, created_at=SCORED_AT
    )
    assert never.active and always.active

    before = score_table(corpus.connection)
    edges = loaded(corpus.connection, NODES)
    adjusted = adjust_edges_with_assertions(
        edges, active_assertions(corpus.connection), nodes=NODES
    )
    after = score_table(corpus.connection)

    assert pairs(edges) == [(CRM_1, CRM_2)], "the scored edge the `never` is about to remove"
    assert pairs(adjusted) == [(BILLING_1, CRM_1)]
    injected = adjusted[0]
    assert injected.match_probability == ASSERTION_EDGE_PROBABILITY == 1.0
    assert injected.evidence == {
        "source": ASSERTION_EVIDENCE_SOURCE,
        "assertion_id": always.assertion_id,
    }
    assert injected.is_assertion

    assert after == before, (
        f"the adjustment changed match_scores: {before} -> {after}. Assertion edges are "
        f"never persisted (S4.4); `assertions` is their durable record"
    )
    assert before.rows == 1 and before.unscored == 0
    assert (
        scalar(
            corpus.connection,
            f"SELECT count(*) FROM {MATCH_SCORES} WHERE rec_a_key = ? AND rec_b_key = ?",
            BILLING_1,
            CRM_1,
        )
        == 0
    ), "the injected pair was never scored, so the edge above came from the assertion alone"
