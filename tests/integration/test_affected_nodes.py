"""The non-batch trigger paths of S4.5.1's affected NODE set, against a real lake.

The unit layer proves the algebra. What only a lake can prove is the part this ticket
exists for: that the four *delta* arms fire at all. Every one of them is a query against
a relation with its own writer and its own timestamp semantics — the assertion delta
against stamps that survive a retraction (S4.4), the deletion arm against tombstone rows
that S4.2 has already removed from the standardized corpus, the content-hash arm against
append-only version history (D7) — and none of those is expressible in a synthetic
dictionary.

**The claim the suite is built around.** S4.5.1 says `er reconcile` "reads the pending
assertion delta itself; this is what makes `split_scenario` executable in incremental
mode when a `never` is asserted on two records that appear in no batch". So
:func:`test_assertion_delta_widens_without_a_batch` delivers **no batch at all**: there
is no `ingest_batches` row for the run, the batch arm is empty by construction, and if
the seed is non-empty it is the assertion arm's doing and nothing else's. The same shape
is used for the review arm. A suite that ingested a batch alongside each delta would
prove nothing about the delta, because the batch arm alone would have produced a
non-empty affected set.

**Why the watermark is read out of the lake rather than off the test's clock.** Every
delta is "since the last successful run", and `raw_records.ingested_at` is stamped by a
separate `er ingest` process. Taking `max(ingested_at)` after the base delivery makes the
cutoff a value the lake itself produced, so no assertion here depends on the two clocks
agreeing or on `TIMESTAMP` being read back in the timezone it was written in.

The `Dbt` harness and the delivery helper are duplicated from
`tests/integration/test_incremental_match.py` rather than imported, for the reason that
module states: a test module importing another test module makes a node id's
dependencies invisible to whoever reads it. Only one test here needs dbt at all —
AC5's claim is precisely about a record that `int_std_records` does not hold.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator, Mapping, Sequence
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
    RECONCILE_STAGE,
    SUCCEEDED,
    AffectedSet,
    affected_nodes,
    current_membership,
    last_reconciled_watermark,
    load_affected_set,
    seed_records,
)
from er.entities.ids import record_key
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.matching.edges import current_edges
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.review.assertions import ALWAYS, NEVER, active_assertions, add_assertion, retract_assertion
from er.review.queue import (
    DISMISS,
    GRAY_BAND,
    NO_MATCH,
    GrayBandPair,
    open_reviews,
    resolve_review,
    upsert_gray_band_pairs,
)

#: The S8.2 fixture whose `base/` phase is the corpus for the two ingest-driven arms.
SCENARIO_NAME: Final = "incremental_batch"
BASE_PHASE: Final = "base"

#: The two dbt selectors `er standardize` runs (S4.2).
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"

#: The scoring key every `match_scores` row here carries. Literal rather than the
#: committed fixture model's, because nothing in this suite scores anything: the edges
#: are inserted directly, exactly as `tests/integration/test_current_edges.py` inserts
#: them, and what is under test is the node set the reader builds over them.
MODEL_VERSION: Final = "v0001"
TF_SNAPSHOT_ID: Final = "01JWMDTFSNAP00000000000069"

#: `thresholds.auto_merge` for this suite. The corpus is never scored, so the value is
#: the test's to choose; the pairs below are placed relative to it.
AUTO_MERGE: Final = 0.90
GRAY_BAND_PROBABILITY: Final = 0.85

#: A fixed watermark for the arms that need no ingest, so their stamps are literal and
#: their ordering is readable rather than inferred from wall-clock ordering.
WATERMARK: Final = datetime(2026, 3, 1, 0, 0, 0)
BEFORE_WATERMARK: Final = WATERMARK - timedelta(hours=1)
AFTER_WATERMARK: Final = WATERMARK + timedelta(hours=1)

#: Two entities, each holding records that appear in no batch and carry no edge. They
#: are the co-members every arm below must widen to.
ENTITY_E: Final = "01JENTITYE0000000000000069"
ENTITY_F: Final = "01JENTITYF0000000000000069"

E_MEMBERS: Final[tuple[str, ...]] = ("billing:B900", "crm:C900", "webforms:W900")
F_MEMBERS: Final[tuple[str, ...]] = ("billing:B901", "crm:C901")

STEWARD: Final = "integration-test"


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


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


def record_run(
    connection: duckdb.DuckDBPyConnection,
    *,
    started_at: datetime,
    stage_status: str = SUCCEEDED,
    run_id: str | None = None,
) -> str:
    """A `runs` row plus its `reconcile` `run_stages` row — the watermark's referent.

    Written directly rather than through `RunContext`, because what the watermark reads
    is the *persisted* pair (S5.2) and this suite has no reconcile stage to run: the
    stage that would write these rows is ER-074's.
    """
    identifier = run_id if run_id is not None else str(ULID())
    insert_row(
        connection,
        "runs",
        {
            "run_id": identifier,
            "tenant": "test",
            "mode": "incremental",
            "status": SUCCEEDED,
            "started_at": started_at,
            "ended_at": started_at + timedelta(minutes=1),
            "config_hash": "0" * 64,
            "std_version": "v1",
            "survivorship_version": "v1",
            "code_version": "test",
        },
    )
    insert_row(
        connection,
        "run_stages",
        {
            "run_id": identifier,
            "stage": RECONCILE_STAGE,
            "seq": 1,
            "status": stage_status,
            "started_at": started_at,
            "ended_at": started_at + timedelta(minutes=1),
        },
    )
    return identifier


def assign(
    connection: duckdb.DuckDBPyConnection,
    entity_id: str,
    members: Sequence[str],
    *,
    run_id: str,
    assigned_at: datetime,
) -> None:
    """`entity_membership` rows for one entity: CURRENT STATE, one row per record (D3)."""
    for key in members:
        source_system, source_record_id = key.split(":", 1)
        insert_row(
            connection,
            "entity_membership",
            {
                "source_system": source_system,
                "source_record_id": source_record_id,
                "record_key": key,
                "entity_id": entity_id,
                "assigned_at": assigned_at,
                "run_id": run_id,
            },
        )


def insert_edge(
    connection: duckdb.DuckDBPyConnection,
    pair: tuple[str, str],
    match_probability: float,
    *,
    run_id: str,
    scored_at: datetime,
) -> None:
    """One active `match_scores` row at this suite's scoring key (S5)."""
    rec_a_key, rec_b_key = pair
    assert rec_a_key < rec_b_key, f"({rec_a_key!r}, {rec_b_key!r}) is not canonical (S5.0)"
    insert_row(
        connection,
        "match_scores",
        {
            "rec_a_key": rec_a_key,
            "rec_b_key": rec_b_key,
            "match_probability": match_probability,
            "model_version": MODEL_VERSION,
            "tf_snapshot_id": TF_SNAPSHOT_ID,
            "rec_a_content_hash": f"{rec_a_key}-hash",
            "rec_b_content_hash": f"{rec_b_key}-hash",
            "evidence": "{}",
            "is_active": True,
            "run_id": run_id,
            "scored_at": scored_at,
        },
    )


def affected(connection: duckdb.DuckDBPyConnection, run_id: str) -> AffectedSet:
    """The affected set for one run, wired the way `er reconcile` will wire it.

    The edge set and the assertion set come from their single readers — ER-059's
    :func:`~er.matching.edges.current_edges` and ER-062's
    :func:`~er.review.assertions.active_assertions` — rather than from queries written
    here, which is the whole reason `load_affected_set` takes them as arguments.
    """
    return load_affected_set(
        connection,
        run_id=run_id,
        auto_merge=AUTO_MERGE,
        edges=current_edges(connection, MODEL_VERSION, TF_SNAPSHOT_ID, min_probability=AUTO_MERGE),
        assertions=active_assertions(connection),
    )


def config() -> Config:
    """The validated S6 document this session runs against (S7.1)."""
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    """The dbt var payload for one config, from S4.2's one generator."""
    payload, _ = blocking_rules_from_config(cfg)
    return payload


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
    """The S8.2 fixture whose `base/` phase this suite ingests."""
    return load_scenario(SCENARIO_NAME)


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


def deliver(rows: Mapping[str, str], root: Path, source: str) -> Path:
    """Write one source's CSV text under the drop-folder layout `er ingest` reads."""
    directory = root / source
    directory.mkdir(parents=True, exist_ok=True)
    for name, text in rows.items():
        (directory / name).write_text(text, encoding="utf-8")
    return root


def ingest(source: str, path: Path, *args: str) -> subprocess.CompletedProcess[str]:
    """One `er ingest` invocation, asserted to have succeeded."""
    result = run_er("ingest", "--source", source, "--path", str(path), *args)
    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
    return result


@dataclass(frozen=True)
class Corpus:
    """A base delivery in the lake, plus the watermark it sits behind."""

    connection: duckdb.DuckDBPyConnection
    scenario: Scenario
    root: Path
    watermark: datetime
    keys: tuple[str, ...]


def crm_rows(scenario: Scenario) -> list[str]:
    """The `base/` crm delivery as its CSV lines, header first."""
    return scenario.inputs_for(BASE_PHASE)["crm"].read_text(encoding="utf-8").splitlines()


def crm_key(line: str) -> str:
    """The S5.0 record key of one crm CSV line; its first column is the record id."""
    return record_key("crm", line.split(",")[0])


@pytest.fixture
def corpus(
    initialised_lake: duckdb.DuckDBPyConnection, scenario: Scenario, tmp_path: Path
) -> Iterator[Corpus]:
    """`base/` ingested for every source, with a succeeded reconcile behind it.

    The watermark is `max(ingested_at)` over what the delivery actually wrote, so
    everything the base phase landed is *before* the last successful run by
    construction — which is what makes any seed this suite computes afterwards
    attributable to the delta it introduced.
    """
    root = tmp_path / "base"
    for source, path in scenario.inputs_for(BASE_PHASE).items():
        deliver({path.name: path.read_text(encoding="utf-8")}, root, source)
        ingest(source, root)

    watermark = scalar(
        initialised_lake, f"SELECT max(ingested_at) FROM {SCHEMA_QUALIFIER}.raw_records"
    )
    assert isinstance(watermark, datetime), "the base delivery wrote no raw_records row"
    record_run(initialised_lake, started_at=watermark)

    keys = tuple(sorted(crm_key(line) for line in crm_rows(scenario)[1:] if line))
    yield Corpus(
        connection=initialised_lake,
        scenario=scenario,
        root=root,
        watermark=watermark,
        keys=keys,
    )


@pytest.fixture
def quiet_lake(initialised_lake: duckdb.DuckDBPyConnection) -> duckdb.DuckDBPyConnection:
    """A lake holding a partition and a reconciled run, and NO ingest at all.

    The substrate for every arm that must fire without a batch: `raw_records` and
    `ingest_batches` are empty, so the batch arm and the two `raw_records` arms are
    empty for any `run_id` whatsoever, and a non-empty seed can only have come from
    `assertions` or `review_queue`.
    """
    reconciled = record_run(initialised_lake, started_at=WATERMARK)
    assign(initialised_lake, ENTITY_E, E_MEMBERS, run_id=reconciled, assigned_at=BEFORE_WATERMARK)
    assign(initialised_lake, ENTITY_F, F_MEMBERS, run_id=reconciled, assigned_at=BEFORE_WATERMARK)
    return initialised_lake


def test_watermark_is_the_last_succeeded_reconcile(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """The cutoff every delta arm shares: the latest run whose `reconcile` succeeded.

    Three runs, deliberately out of insertion order, and one of them failed. A watermark
    that read `max(started_at)` over `runs` alone would return the failed run's instant
    and silently drop every change made since the last run that actually reconciled.
    """
    assert last_reconciled_watermark(initialised_lake) is None, (
        "a lake that has never reconciled has no cutoff; every delta arm is unbounded"
    )

    record_run(initialised_lake, started_at=BEFORE_WATERMARK)
    record_run(initialised_lake, started_at=WATERMARK)
    record_run(initialised_lake, started_at=AFTER_WATERMARK, stage_status="failed")

    assert last_reconciled_watermark(initialised_lake) == WATERMARK


def test_assertion_delta_widens_without_a_batch(quiet_lake: duckdb.DuckDBPyConnection) -> None:
    """AC3: a `never` on two non-batch records yields both entities' full membership.

    The ticket's reason for existing, stated as a test. No delivery is made, so the run
    has no `ingest_batches` row and the batch arm is empty; the only thing that changed
    since the last successful reconcile is one steward assertion between a member of
    entity E and a member of entity F, neither of which any batch has ever touched.

    `never` rather than `always` on purpose: a `never` supplies no partner (S4.4 removes
    the edge), so the widening cannot be attributed to the partner rule. Both entities
    arrive through the assertion arm of the SEED and one hop of membership, which is
    exactly the path `split_scenario` needs in incremental mode.
    """
    run_id = str(ULID())
    empty = affected(quiet_lake, run_id)
    assert empty.is_empty, "the substrate is not quiet; the assertion below proves nothing"

    add_assertion(
        quiet_lake,
        a="crm:C900",
        b="crm:C901",
        kind=NEVER,
        created_by=STEWARD,
        created_at=AFTER_WATERMARK,
    )

    result = affected(quiet_lake, run_id)

    assert result.seed == frozenset({"crm:C900", "crm:C901"})
    assert result.partners == frozenset(), "a `never` removes the edge; it supplies no partner"
    assert result.entities == frozenset({ENTITY_E, ENTITY_F})
    assert result.nodes == frozenset(E_MEMBERS) | frozenset(F_MEMBERS)
    assert not result.is_empty

    # The batch arm really is empty, so the seed above is the assertion arm's alone.
    arms = seed_records(quiet_lake, run_id=run_id, watermark=WATERMARK)
    assert arms.batch == frozenset()
    assert arms.assertion_delta == frozenset({"crm:C900", "crm:C901"})
    assert arms.arm_sizes() == {
        "batch": 0,
        "content_hash_delta": 0,
        "deletions": 0,
        "assertion_delta": 2,
        "review_delta": 0,
    }


def test_retraction_is_a_delta_too(quiet_lake: duckdb.DuckDBPyConnection) -> None:
    """A retraction since the watermark seeds its endpoints, though nothing is active.

    S4.4 keeps retracted rows precisely so the delta is computable, and S4.4.2's cut is
    released only when its assertion is retracted — a change to the partition that no
    other arm surfaces. An implementation reading `active_assertions` for the delta
    passes AC3 and fails this.
    """
    written = add_assertion(
        quiet_lake,
        a="crm:C900",
        b="crm:C901",
        kind=ALWAYS,
        created_by=STEWARD,
        created_at=BEFORE_WATERMARK,
    )
    run_id = str(ULID())

    # Created before the watermark, still active: not a delta, and its endpoints reach
    # the seed through no arm. It IS an active `always`, so it still supplies a partner —
    # which is the edge set's business, not the seed's.
    settled = seed_records(quiet_lake, run_id=run_id, watermark=WATERMARK)
    assert settled.assertion_delta == frozenset()

    retract_assertion(
        quiet_lake, written.assertion_id, retracted_by=STEWARD, retracted_at=AFTER_WATERMARK
    )

    delta = seed_records(quiet_lake, run_id=run_id, watermark=WATERMARK)
    assert delta.assertion_delta == frozenset({"crm:C900", "crm:C901"})
    assert active_assertions(quiet_lake) == [], "the row is retracted, and still in the delta"

    result = affected(quiet_lake, run_id)
    assert result.entities == frozenset({ENTITY_E, ENTITY_F})
    assert result.nodes == frozenset(E_MEMBERS) | frozenset(F_MEMBERS)


def test_review_resolution_seed(quiet_lake: duckdb.DuckDBPyConnection) -> None:
    """AC6: a `review_queue` row resolved after the watermark seeds both endpoints.

    Resolved as `dismiss`, which writes no `assertions` row (S4.3.5) — so the endpoints
    can have reached the seed through the review arm and through nothing else. The
    `no_match` arm is asserted alongside it on a second pair, because a resolution that
    *does* write an assertion must still be the review arm's own responsibility.
    """
    run_id = str(ULID())
    upsert_gray_band_pairs(
        quiet_lake,
        [
            GrayBandPair(
                rec_a_key=rec_a_key,
                rec_b_key=rec_b_key,
                match_probability=GRAY_BAND_PROBABILITY,
                waterfall={"gamma_email": 2, "bf_email": 12.5},
            )
            for rec_a_key, rec_b_key in (
                ("crm:C900", "webforms:W900"),
                ("billing:B901", "crm:C901"),
            )
        ],
        run_id=run_id,
    )

    queued = open_reviews(quiet_lake)
    assert len(queued) == 2 and {row.reason for row in queued} == {GRAY_BAND}
    assert seed_records(quiet_lake, run_id=run_id, watermark=WATERMARK).review_delta == frozenset()

    by_pair = {(row.rec_a_key, row.rec_b_key): row.review_id for row in queued}
    resolve_review(
        quiet_lake,
        by_pair[("crm:C900", "webforms:W900")],
        resolution=DISMISS,
        resolved_by=STEWARD,
        resolved_at=AFTER_WATERMARK,
    )
    resolve_review(
        quiet_lake,
        by_pair[("billing:B901", "crm:C901")],
        resolution=NO_MATCH,
        resolved_by=STEWARD,
        resolved_at=AFTER_WATERMARK,
    )

    arms = seed_records(quiet_lake, run_id=run_id, watermark=WATERMARK)
    assert arms.review_delta == frozenset({"crm:C900", "webforms:W900", "billing:B901", "crm:C901"})
    # The dismissed pair writes no assertion, so its two endpoints are in the review arm
    # and in no other — which is what makes the arm's contribution attributable.
    assert not {"crm:C900", "webforms:W900"} & arms.assertion_delta

    result = affected(quiet_lake, run_id)
    assert result.entities == frozenset({ENTITY_E, ENTITY_F})
    assert result.nodes == frozenset(E_MEMBERS) | frozenset(F_MEMBERS)


def test_unchanged_rerun_is_empty(quiet_lake: duckdb.DuckDBPyConnection) -> None:
    """AC7: no ingest, no assertion delta, no hash change, no deletion → nothing affected.

    The negative control for the whole suite, and S4.0's exit ``10`` for `er reconcile`
    (ER-074's to wire). The lake is not empty — it holds two entities, five membership
    rows and a scored edge above `auto_merge` — so an implementation that seeded from the
    edge set, or from `entity_membership`, would report the whole corpus here.
    """
    insert_edge(
        quiet_lake,
        ("crm:C900", "webforms:W900"),
        0.99,
        run_id=str(ULID()),
        scored_at=BEFORE_WATERMARK,
    )
    add_assertion(
        quiet_lake,
        a="billing:B900",
        b="crm:C900",
        kind=ALWAYS,
        created_by=STEWARD,
        created_at=BEFORE_WATERMARK,
    )

    result = affected(quiet_lake, str(ULID()))

    assert result.nodes == frozenset()
    assert result.entities == frozenset()
    assert result.seed == frozenset()
    assert result.partners == frozenset()
    assert result.is_empty

    # And the substrate the empty answer was computed over really does hold rows.
    assert scalar(quiet_lake, f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.entity_membership") == len(
        E_MEMBERS
    ) + len(F_MEMBERS)
    assert current_edges(quiet_lake, MODEL_VERSION, TF_SNAPSHOT_ID, min_probability=AUTO_MERGE)


def test_content_hash_delta_widens_to_full_membership(corpus: Corpus) -> None:
    """AC4: a re-delivered record with a new `content_hash` widens to its whole entity.

    One crm record is re-delivered with a changed email and nothing else changes. The
    other members of its entity appear in no delivery at all — they are affected because
    S4.5.5 says a superseded record's entity "widens through the affected-edge rule to
    that entity's full membership", and INV-SCORE (S4.3.3) is stated over the endpoint
    hashes for the same reason: the pair is now a different scoring problem.

    A record delivered again at its KNOWN hash is the control. S4.1 counts that
    `unchanged` and appends nothing, so it must not enter the seed — otherwise every
    idempotent re-delivery would re-cluster the corpus.
    """
    header, *rows = crm_rows(corpus.scenario)
    changed_key = crm_key(rows[0])
    unchanged_key = crm_key(rows[1])
    assert changed_key in corpus.keys and unchanged_key in corpus.keys

    assign(
        corpus.connection,
        ENTITY_E,
        (changed_key, *E_MEMBERS),
        run_id=str(ULID()),
        assigned_at=corpus.watermark,
    )

    fields = rows[0].split(",")
    fields[3] = "moved.address@example.com"
    revised = "\n".join([header, ",".join(fields), rows[1]]) + "\n"
    deliver({"crm.csv": revised}, corpus.root / "revised", "crm")
    ingest("crm", corpus.root / "revised")

    run_id = str(ULID())
    arms = seed_records(corpus.connection, run_id=run_id, watermark=corpus.watermark)

    assert changed_key in arms.content_hash_delta
    assert unchanged_key not in arms.content_hash_delta, (
        f"{unchanged_key} was re-delivered at its known content_hash; S4.1 counts that "
        f"`unchanged` and appends no row, so it is not a delta"
    )
    assert arms.deletions == frozenset(), "nothing was tombstoned or resurrected"

    result = affected(corpus.connection, run_id)
    assert result.entities == frozenset({ENTITY_E})
    assert result.nodes >= frozenset({changed_key, *E_MEMBERS}), (
        "the changed record's entity did not widen to its full membership (S4.5.5)"
    )
    for co_member in E_MEMBERS:
        assert co_member not in arms.records, (
            f"{co_member} is in the SEED, not merely in the affected set; it appears in "
            f"no delivery and its own content is unchanged"
        )

    # `current_membership` is what supplied the co-members, and it loads the whole
    # entity rather than only the records it was asked about.
    loaded = current_membership(corpus.connection, [changed_key])
    assert set(loaded) == {changed_key, *E_MEMBERS}
    assert set(loaded.values()) == {ENTITY_E}


def test_tombstone_and_resurrection_seed(corpus: Corpus, cfg: Config, dbt_packages: None) -> None:
    """AC5: a tombstoned key seeds even though `int_std_records` no longer holds it.

    The arm that cannot read the standardized corpus. `er ingest --full-refresh-keys`
    treats the delivery as the complete key set for `crm` (S4.1.1), so the omitted key
    gains a tombstone version; S4.2 then excludes it from `int_std_records` entirely.
    A deletions arm reading that view would find nothing — which is why the arm reads
    `raw_records`, and why this test standardizes at all: the absence is the point.

    The resurrection half follows on the same key, so the two directions of one event
    class are asserted against the same record rather than against two arranged ones.
    """
    dbt = Dbt(connection=corpus.connection, artifacts=corpus.root.parent / "artifacts", cfg=cfg)
    dbt("seed")

    header, *rows = crm_rows(corpus.scenario)
    tombstoned_key = crm_key(rows[0])
    surviving_key = crm_key(rows[1])

    assign(
        corpus.connection,
        ENTITY_E,
        (tombstoned_key, *E_MEMBERS),
        run_id=str(ULID()),
        assigned_at=corpus.watermark,
    )

    # The complete key set for `crm`, minus one row.
    trimmed = "\n".join([header, *(line for line in rows if crm_key(line) != tombstoned_key)])
    deliver({"crm.csv": trimmed + "\n"}, corpus.root / "refresh", "crm")
    ingest("crm", corpus.root / "refresh", "--full-refresh-keys")
    dbt.standardize()

    standardized = {
        str(key)
        for (key,) in corpus.connection.execute(f"SELECT record_key FROM {STD_RECORDS}").fetchall()
    }
    assert tombstoned_key not in standardized, (
        f"{tombstoned_key} is still in int_std_records; S4.2 excludes a tombstoned record "
        f"from the standardized corpus, and this arm exists because of that exclusion"
    )
    assert surviving_key in standardized, "the refresh removed more than the omitted key"

    run_id = str(ULID())
    buried = seed_records(corpus.connection, run_id=run_id, watermark=corpus.watermark)
    assert tombstoned_key in buried.deletions
    assert surviving_key not in buried.deletions

    result = affected(corpus.connection, run_id)
    assert result.entities == frozenset({ENTITY_E})
    assert result.nodes >= frozenset({tombstoned_key, *E_MEMBERS}), (
        "the tombstoned record's entity did not widen to its full membership (S4.5.5): "
        "member_removed / split / retired are unreachable without it"
    )

    # Resurrection: the key re-appears in an ordinary delivery, with no special case.
    # It carries a NEW content value, because the append is an anti-join on
    # `(source_system, source_record_id, content_hash)` (S4.1) and re-delivering a
    # version byte-identical to one already in the history appends nothing at all.
    revived_row = rows[0].split(",")
    revived_row[3] = "back.again@example.com"
    deliver(
        {"crm.csv": "\n".join([header, ",".join(revived_row), *rows[1:]]) + "\n"},
        corpus.root / "resurrect",
        "crm",
    )
    ingest("crm", corpus.root / "resurrect")

    revived = seed_records(corpus.connection, run_id=run_id, watermark=corpus.watermark)
    assert tombstoned_key in revived.deletions, (
        "a resurrected key is an ordinary content version appended after a tombstone "
        "(S4.1.1); it re-enters the seed by the same arm that buried it"
    )
    assert (
        scalar(
            corpus.connection,
            f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.raw_records "
            f"WHERE source_system = 'crm' AND source_record_id = ? AND is_deleted",
            tombstoned_key.split(":", 1)[1],
        )
        == 1
    ), "S4.1.1: resurrection appends a version and never rewrites the tombstone"


def test_batch_arm_is_scoped_to_this_run(corpus: Corpus) -> None:
    """The batch arm reads `ingest_batches` by `run_id`, not the whole relation.

    The corpus was delivered by earlier runs, so a batch arm that read every batch would
    return the entire corpus on a run that ingested nothing — and every other arm's
    contribution would be invisible underneath it.
    """
    stale = seed_records(corpus.connection, run_id=str(ULID()), watermark=corpus.watermark)
    assert stale.batch == frozenset()

    delivered = str(
        scalar(
            corpus.connection,
            f"SELECT run_id FROM {SCHEMA_QUALIFIER}.ingest_batches "
            f"WHERE source_system = 'crm' ORDER BY created_at DESC LIMIT 1",
        )
    )
    mine = seed_records(corpus.connection, run_id=delivered, watermark=corpus.watermark)
    assert mine.batch == frozenset(corpus.keys), (
        "the batch arm did not return exactly the crm delivery of the run that made it"
    )

    # And the widening still applies to a batch record: with no membership written, the
    # seed is the affected set.
    result = affected_nodes(mine.records, edges=[], membership={}, auto_merge=AUTO_MERGE)
    assert result.nodes == mine.batch
    assert result.entities == frozenset()


def test_partner_widening_uses_edges_not_the_batch(quiet_lake: duckdb.DuckDBPyConnection) -> None:
    """A scored partner brings its own entity in; a gray-band pair does not (S4.5.1).

    Against the real reader — :func:`~er.matching.edges.current_edges` at
    `min_probability=auto_merge` — rather than a literal edge list, so the inclusive
    bound is asserted where it is actually applied: at the query, over rows written into
    `match_scores` under this run's `(model_version, tf_snapshot_id)`.
    """
    scoring_run = str(ULID())
    insert_edge(
        quiet_lake, ("crm:C900", "crm:C901"), AUTO_MERGE, run_id=scoring_run, scored_at=WATERMARK
    )
    insert_edge(
        quiet_lake,
        ("billing:B900", "billing:B901"),
        GRAY_BAND_PROBABILITY,
        run_id=scoring_run,
        scored_at=WATERMARK,
    )
    add_assertion(
        quiet_lake,
        a="crm:C900",
        b="webforms:W900",
        kind=ALWAYS,
        created_by=STEWARD,
        created_at=AFTER_WATERMARK,
    )

    run_id = str(ULID())
    result = affected(quiet_lake, run_id)

    # The seed is the assertion's two endpoints, both already in entity E.
    assert result.seed == frozenset({"crm:C900", "webforms:W900"})
    assert result.partners == frozenset({"crm:C901"}), (
        "the pair at exactly auto_merge supplies a partner and the banded pair does not; "
        "a banded pair is queued for review and is not clustered (S4.3.5)"
    )
    assert result.entities == frozenset({ENTITY_E, ENTITY_F})
    assert result.nodes == frozenset(E_MEMBERS) | frozenset(F_MEMBERS)

    # The banded pair's own endpoints are in the node set only as co-members, never as
    # partners — `billing:B901` arrives through entity F, which `crm:C901` opened.
    assert "billing:B901" not in result.partners
