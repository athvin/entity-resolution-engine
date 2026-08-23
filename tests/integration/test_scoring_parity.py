"""T-INC-3 scoring parity and T-MATCH-SYM orientation invariance (S8.3, S4.3.4, S4.2).

**Why T-INC-3 exists.** T-INC-1 compares two independently built universes and asserts
they induce the same partition. When it fails, the cause is either scoring or
clustering, and the two are a long way apart. T-INC-3 is the oracle that decides:
if the same pairs score bit-equally through the incremental two-pass path and through
the corpus-wide `full.py` pass, then a T-INC-1 failure is clustering's, and nobody
spends a day in Splink. That only works while the comparison is exact — a tolerance
here would let scoring drift under the oracle that exists to exonerate it. So the
assertion is `==` on the DOUBLE. S8.2.1's `1e-9` is the tolerance for *expected-file*
comparison and has no bearing on this one.

**The two arms are separate universes, and they have to be.** `tests/conftest.py`'s
`sub_namespace` is explicit: two universes inside one namespace share `runs` and
`run_stages`, so the incremental arm would read the full arm's stage rows. Each arm is
therefore built in its own S7.2 namespace. Because `er ingest` and dbt are subprocesses
and :func:`~er.lake.ducklake.attach_statements` renders from the process environment,
each arm's work happens inside a scoped `os.environ` patch — exported just long enough,
which is what `sub_namespace` itself does during construction. Leaving it exported
would silently re-point the session.

**What counts as "scored by the incremental path".** The incremental arm's
`match_scores` is cumulative: a base full run, then the two-pass batch run. Filtering by
the batch run's own `run_id` is what separates the pass's output from the base rows it
sits beside — without that filter every base-only pair would look like something the
two-pass path produced, and the S8.2.1 endpoint theorem (no pair of two base records is
reachable from `find_matches_to_new_records` or a batch-only `dedupe_only` linker) would
appear to be violated by the fixture.

**T-MATCH-SYM and why it is provable at all.** `compare_two_records(a, b)` and
`compare_two_records(b, a)` must agree, and the only comparison level that could
plausibly break that is `variant_match` — it reads one record's `name_variants` array
against the other's `given_name`. S4.2 guarantees the normalized `given_name` is element
0 of its own array, which makes the relation symmetric. That precondition is asserted
here as its own test rather than assumed, because if it ever stopped holding, the
orientation test would fail with no indication of why.

`1e-12` rather than `==` for the orientation arm is deliberate and is the ticket's:
`compare_two_records` runs each orientation through its own SQL pipeline, so the two
are not guaranteed to be the same floating-point *operations* in the same order, and an
exact comparison would be asserting something about DuckDB's expression evaluation
rather than about the model.

**A disagreement here is a blocker, not an edit.** `src/er/matching/incremental.py` and
`src/er/matching/full.py` are both `protected_paths` of this ticket: if the two paths
disagree, the ticket blocks against them rather than adjusting the implementations it
exists to test (AC8, structurally enforced — `plan-check` refuses a plan naming them).

The `Dbt` harness and the delivery helper are duplicated from
`tests/integration/test_incremental_match.py` rather than imported, for the reason that
module states: a test module importing another test module makes a node id's
dependencies invisible to whoever reads it.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator, Mapping
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final
from unittest import mock

import duckdb
import pytest
from helpers.model import fixture_settings, load_fixture_model
from helpers.pairs import canonical_pairs_from_blocking_keys
from helpers.parity import (
    PARITY_FILENAME,
    REGEN_ENV,
    Pair,
    derive_parity_pairs,
    read_parity_pairs,
    symmetric_difference_report,
    write_parity_pairs,
)
from helpers.scenario import Scenario, load_scenario
from splink import Linker
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import (
    ARTIFACTS_DIR,
    DBT_PROFILES_DIR,
    DBT_PROJECT_DIR,
    DbtResult,
    render_dbt_vars,
    run_dbt,
)
from er.entities.ids import canonicalize_pair, record_key
from er.errors import ExitCode
from er.lake.columns import STD_RECORD_COLUMNS
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.api import splink_api
from er.matching.full import MATCH_SCORES_RELATION, score_full
from er.matching.incremental import score_incremental
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.matching.tf import register_tf
from er.obs.counters import DECLARED_COUNTERS, StageCounters
from er.obs.runctx import StageRun

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

#: The two scenarios: the parity arms run over `incremental_batch`, T-MATCH-SYM over
#: `base_10` (S8.2 gives the latter the designed comparison-level traps).
PARITY_SCENARIO: Final = "incremental_batch"
SYMMETRY_SCENARIO: Final = "base_10"

BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.{MATCH_SCORES_RELATION}"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

MATCH_STAGE: Final = "match"

#: AC6's bound. Not `==`: each orientation runs through its own `compare_two_records`
#: pipeline, so exactness would be a claim about DuckDB's evaluation order.
ORIENTATION_TOLERANCE: Final = 1e-12

#: The bare local relation the symmetry corpus is copied into, distinct from the
#: scorer's and the parity helper's for the reason `helpers.pairs` gives: Splink
#: resolves an input table by bare name, and two suites in one connection must not
#: replace each other's corpus.
SYMMETRY_CORPUS_RELATION: Final = "er_symmetry_corpus"

PARITY_PATH: Final = REPO_ROOT / "fixtures" / "static" / PARITY_SCENARIO / PARITY_FILENAME


def config() -> Config:
    """The validated S6 document this session runs against (S7.1)."""
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    """The dbt var payload for one config, from S4.2's one generator."""
    payload, _ = blocking_rules_from_config(cfg)
    return payload


@contextmanager
def pointing_at(env: Mapping[str, str]) -> Iterator[None]:
    """Point the process at one universe for the duration of a block.

    `er ingest` and dbt are subprocesses and `attach_statements` renders from the
    process environment, so an arm's ingest, standardize and reattach all have to see
    its own S7.2 pair. The patch is scoped rather than exported for the reason
    `sub_namespace` gives: the environment can name only one universe, and leaving one
    named would re-point the session and make a later `er init` apply to the wrong lake.
    """
    with mock.patch.dict(os.environ, dict(env)):
        yield


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script against whatever env is current."""
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


def scored_pairs(
    connection: duckdb.DuckDBPyConnection, *, run_id: str | None = None
) -> dict[Pair, float]:
    """`match_scores` as canonical pair -> probability, optionally for one run only."""
    statement = f"SELECT rec_a_key, rec_b_key, match_probability FROM {MATCH_SCORES}"
    parameters: list[Any] = []
    if run_id is not None:
        statement += " WHERE run_id = ?"
        parameters.append(run_id)
    rows = connection.execute(statement, parameters).fetchall()
    return {
        canonicalize_pair(str(rec_a), str(rec_b)): float(probability)
        for rec_a, rec_b, probability in rows
    }


def active_pins(connection: duckdb.DuckDBPyConnection) -> tuple[str, str]:
    """The `(model_version, tf_snapshot_id)` of this universe's active registry row."""
    row = connection.execute(
        f"SELECT model_version, tf_snapshot_id FROM {MODEL_REGISTRY} WHERE status = 'active'"
    ).fetchone()
    assert row is not None, "no active model_registry row; the arm scored at nothing"
    return str(row[0]), str(row[1])


def batch_record_keys(scenario: Scenario) -> frozenset[str]:
    """The `record_key`s of the `batch/` delivery, from the fixture's own CSVs.

    Derived from the fixture rather than from `unscored_record_keys`, so the endpoint
    condition is a claim about the delivery and not a restatement of whatever the
    implementation chose to score.
    """
    keys: list[str] = []
    for source, path in scenario.inputs_for(BATCH_PHASE).items():
        lines = path.read_text(encoding="utf-8").splitlines()
        keys += [record_key(source, line.split(",")[0]) for line in lines[1:] if line]
    return frozenset(keys)


def new_stage_run() -> StageRun:
    """A `run_stages` row for one in-process stage."""
    return StageRun(
        run_id=str(ULID()),
        stage=MATCH_STAGE,
        seq=1,
        started_at=datetime.now(UTC),
        counters=StageCounters(DECLARED_COUNTERS[MATCH_STAGE]),
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


@dataclass(frozen=True)
class Parity:
    """Both arms' results, and the pins that make comparing them meaningful."""

    incremental: dict[Pair, float]
    full: dict[Pair, float]
    batch_keys: frozenset[str]
    incremental_pins: tuple[str, str]
    full_pins: tuple[str, str]


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return config()


@pytest.fixture
def parity_scenario() -> Scenario:
    """`incremental_batch`, which is the only scenario with a `batch/` phase here."""
    scenario = load_scenario(PARITY_SCENARIO)
    assert BATCH_PHASE in scenario.phases, f"{PARITY_SCENARIO} has no {BATCH_PHASE} phase"
    return scenario


@pytest.fixture
def base_10() -> Scenario:
    """The S8.2 fixture T-MATCH-SYM runs over."""
    return load_scenario(SYMMETRY_SCENARIO)


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


def _ingest(scenario: Scenario, phase: str, root: Path) -> None:
    """Deliver and ingest one phase against the currently pointed-at universe."""
    delivery = deliver(scenario, phase, root / phase)
    for source in scenario.inputs_for(phase):
        result = run_er("ingest", "--source", source, "--path", str(delivery))
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr


@pytest.fixture
def parity(
    parity_scenario: Scenario,
    dbt_packages: None,
    sub_namespace: Any,
    cfg: Config,
    tmp_path: Path,
) -> Parity:
    """Both arms, each in its own universe, scored at the same committed model.

    Arm A is the pipeline as it actually runs incrementally: a base full run, then the
    two-pass batch run. Arm B is the corpus-wide pass over `base/ ∪ batch/`. Both load
    the same committed `model_test_v1`, so the `(model_version, tf_snapshot_id)` pins
    the tests assert are equal by construction — and asserted anyway, because "the two
    arms were pinned identically" is the premise the whole comparison rests on.
    """
    incremental_universe = sub_namespace("inc")
    full_universe = sub_namespace("full")

    with pointing_at(incremental_universe.env):
        dbt = Dbt(incremental_universe.connection, tmp_path / "artifacts_inc", cfg)
        dbt("seed")
        _ingest(parity_scenario, BASE_PHASE, tmp_path / "drop_inc")
        dbt.standardize()
        model_version, tf_snapshot_id, _ = load_fixture_model(incremental_universe.connection)
        score_full(
            incremental_universe.connection,
            cfg,
            new_stage_run(),
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            settings=fixture_settings(),
        )
        _ingest(parity_scenario, BATCH_PHASE, tmp_path / "drop_inc")
        dbt.standardize()
        batch_run = new_stage_run()
        score_incremental(
            incremental_universe.connection,
            cfg,
            batch_run,
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            settings=fixture_settings(),
        )
        # Only the two-pass run's own rows: the base run's pairs sit in the same
        # relation and are not something the incremental path produced.
        incremental_scores = scored_pairs(incremental_universe.connection, run_id=batch_run.run_id)
        incremental_pins = active_pins(incremental_universe.connection)

    with pointing_at(full_universe.env):
        dbt_full = Dbt(full_universe.connection, tmp_path / "artifacts_full", cfg)
        dbt_full("seed")
        _ingest(parity_scenario, BASE_PHASE, tmp_path / "drop_full")
        _ingest(parity_scenario, BATCH_PHASE, tmp_path / "drop_full")
        dbt_full.standardize()
        full_version, full_snapshot, _ = load_fixture_model(full_universe.connection)
        score_full(
            full_universe.connection,
            cfg,
            new_stage_run(),
            model_version=full_version,
            tf_snapshot_id=full_snapshot,
            settings=fixture_settings(),
        )
        full_scores = scored_pairs(full_universe.connection)
        full_pins = active_pins(full_universe.connection)

    assert incremental_scores, "the incremental pass scored nothing; parity would be vacuous"
    assert full_scores, "the full pass scored nothing; parity would be vacuous"
    return Parity(
        incremental=incremental_scores,
        full=full_scores,
        batch_keys=batch_record_keys(parity_scenario),
        incremental_pins=incremental_pins,
        full_pins=full_pins,
    )


def test_parity_pairs_file_matches_derivation(parity: Parity) -> None:
    """AC1/AC4/AC5: the committed set equals the derivation, or the diff says how not.

    S8.2.1 makes the file DERIVED, so this is the assertion that keeps it honest. It
    deliberately says nothing about cardinality — the title's "50 pairs" is scale, not
    a contract — and everything about *which* pairs, because a silent shrink is the
    failure mode a count would hide.
    """
    derived = derive_parity_pairs(parity.incremental, parity.full, parity.batch_keys)
    assert derived, (
        "the derived parity set is empty: no pair was scored by both paths with an "
        "endpoint in batch/, so T-INC-3 would assert nothing"
    )

    if os.environ.get(REGEN_ENV):
        written = write_parity_pairs(PARITY_PATH, derived)
        # S7.1's image COPYs the repository rather than bind-mounting it, so a file
        # written at PARITY_PATH inside the container never reaches the host. Only
        # `artifacts/` is mounted, so the copy there is how a containerised
        # regeneration is actually collected; a developer running pytest against a
        # working tree gets the real file and can ignore it.
        escaped = ARTIFACTS_DIR / PARITY_FILENAME
        if ARTIFACTS_DIR.is_dir():
            write_parity_pairs(escaped, derived)
        pytest.fail(
            f"{REGEN_ENV} was set: rewrote {PARITY_PATH} with {written} pair(s) in "
            f"S8.2.1 sort order, and copied it to {escaped} so a containerised run can "
            "collect it. This failure is deliberate — regeneration must never leave a "
            "green suite, or a parity regression becomes a self-healing no-op. Review "
            "the diff and re-run without the variable."
        )

    assert PARITY_PATH.is_file(), (
        f"{PARITY_PATH} is not committed. It is DERIVED (S8.2.1), so generate it with "
        f"{REGEN_ENV}=1 and commit the result rather than authoring it by hand."
    )
    committed = read_parity_pairs(PARITY_PATH)
    assert derived == committed, symmetric_difference_report(derived, committed)


def test_incremental_and_full_scores_are_bit_equal(parity: Parity) -> None:
    """AC2/AC3: every parity pair scores identically on both paths.

    The pins are asserted first, and that ordering is the point: if the two arms had
    scored at different models or different TF snapshots, a probability difference
    would be expected rather than a defect, and the failure would be attributed to the
    wrong thing (S4.3.3, D4).
    """
    assert parity.incremental_pins == parity.full_pins, (
        f"the arms were pinned differently — incremental {parity.incremental_pins}, "
        f"full {parity.full_pins}; a probability difference under different pins says "
        "nothing about the two code paths"
    )

    pairs = sorted(derive_parity_pairs(parity.incremental, parity.full, parity.batch_keys))
    assert pairs, "no parity pairs; the comparison below would be vacuous"

    for pair in pairs:
        # AC2: the endpoint condition, per pair, so a failure names the pair.
        assert pair[0] in parity.batch_keys or pair[1] in parity.batch_keys, (
            f"{pair} has no endpoint in batch/ yet reached the parity set"
        )
    mismatched = [
        (pair, parity.incremental[pair], parity.full[pair])
        for pair in pairs
        if parity.incremental[pair] != parity.full[pair]
    ]
    assert not mismatched, (
        f"{len(mismatched)} of {len(pairs)} parity pairs disagree between the "
        "incremental two-pass path and the corpus-wide pass at one "
        f"{parity.full_pins}. Bit-equality is what lets a T-INC-1 failure be attributed "
        "to clustering (S8.3, T-INC-3):\n"
        + "\n".join(
            f"  {a} | {b}: incremental={incremental!r} full={full!r} "
            f"delta={incremental - full:+.3e}"
            for (a, b), incremental, full in mismatched[:20]
        )
    )


@dataclass
class SymmetryCorpus:
    """`base_10` standardized, with a linker able to score one pair at a time."""

    connection: duckdb.DuckDBPyConnection
    linker: Linker
    records: dict[str, dict[str, Any]]
    blocked: set[Pair]


@pytest.fixture
def symmetry(
    base_10: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[SymmetryCorpus]:
    """`base_10` ingested and standardized, with the committed model registered.

    The teardown restores the connection's default schema: `splink_api` repoints the
    shared session handle at the scratch schema.
    """
    dbt = Dbt(initialised_lake, tmp_path / "artifacts_sym", cfg)
    dbt("seed")
    _ingest(base_10, BASE_PHASE, tmp_path / "drop_sym")
    dbt.standardize()

    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        model_version, tf_snapshot_id, settings = load_fixture_model(initialised_lake)
        api = splink_api(initialised_lake)
        initialised_lake.execute(
            f"CREATE OR REPLACE TABLE {SYMMETRY_CORPUS_RELATION} AS "
            f"SELECT {', '.join(STD_RECORD_COLUMNS)} FROM {STD_RECORDS}"
        )
        linker = Linker(SYMMETRY_CORPUS_RELATION, settings=settings, db_api=api)
        # The frozen TF, as any scoring path registers it (D4): an unregistered linker
        # would compare the two orientations under term frequencies computed twice.
        register_tf(linker, initialised_lake, cfg, model_version, tf_snapshot_id)

        rows = initialised_lake.execute(
            f"SELECT {', '.join(STD_RECORD_COLUMNS)} FROM {STD_RECORDS}"
        ).fetchall()
        records = {str(row[0]): dict(zip(STD_RECORD_COLUMNS, row, strict=True)) for row in rows}
        yield SymmetryCorpus(
            connection=initialised_lake,
            linker=linker,
            records=records,
            blocked=canonical_pairs_from_blocking_keys(initialised_lake),
        )
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def _probability(linker: Linker, left: Mapping[str, Any], right: Mapping[str, Any]) -> float:
    """`compare_two_records` for one ordered pair, as a probability."""
    frame = linker.inference.compare_two_records(dict(left), dict(right))
    rows = frame.as_record_dict()
    assert len(rows) == 1, f"compare_two_records returned {len(rows)} rows, expected 1"
    return float(rows[0]["match_probability"])


def test_score_is_orientation_invariant(symmetry: SymmetryCorpus) -> None:
    """AC6/T-MATCH-SYM: `compare(a,b)` and `compare(b,a)` agree on every blocked pair.

    Every blocked pair, not a sample: the comparison levels that could break symmetry
    are data-dependent, and a sample chosen by anything other than the data would miss
    exactly the record whose `name_variants` array is malformed.
    """
    assert symmetry.blocked, "no blocked pairs on base_10; the assertion would be vacuous"

    asymmetric: list[tuple[Pair, float, float]] = []
    for pair in sorted(symmetry.blocked):
        left, right = symmetry.records[pair[0]], symmetry.records[pair[1]]
        forward = _probability(symmetry.linker, left, right)
        backward = _probability(symmetry.linker, right, left)
        if abs(forward - backward) > ORIENTATION_TOLERANCE:
            asymmetric.append((pair, forward, backward))

    assert not asymmetric, (
        f"{len(asymmetric)} of {len(symmetry.blocked)} blocked pairs score differently "
        f"depending on argument order (tolerance {ORIENTATION_TOLERANCE:g}). Scoring "
        "must not depend on which record is called `l` (S4.2, T-MATCH-SYM):\n"
        + "\n".join(
            f"  {a} | {b}: forward={forward!r} backward={backward!r} "
            f"delta={forward - backward:+.3e}"
            for (a, b), forward, backward in asymmetric[:20]
        )
    )


def test_name_variants_element_zero_symmetry(symmetry: SymmetryCorpus) -> None:
    """AC7: the normalized `given_name` is element 0 of its own `name_variants`.

    This is the precondition `variant_match` orientation independence rests on (S4.2).
    `variant_match` reads one record's array against the other's `given_name`; if a
    record's own normalized name were absent from — or not first in — its array, the
    relation would hold in one direction and not the other, and
    `test_score_is_orientation_invariant` would fail with no indication of the cause.
    """
    offenders: list[tuple[str, Any, Any]] = []
    for key, row in sorted(symmetry.records.items()):
        given_name, variants = row["given_name"], row["name_variants"]
        if given_name is None:
            # A NULL `given_name` has no variants to lead; `NullLevel` handles the pair
            # and no orientation question arises.
            continue
        listed = list(variants) if variants is not None else []
        if not listed or listed[0] != given_name:
            offenders.append((key, given_name, listed))

    assert not offenders, (
        "the normalized given_name is not element 0 of its own name_variants array, so "
        "`variant_match` is not symmetric for these records (S4.2):\n"
        + "\n".join(
            f"  {key}: given_name={given_name!r} name_variants={listed!r}"
            for key, given_name, listed in offenders[:20]
        )
    )
