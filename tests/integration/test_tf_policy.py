"""D4's behavioural arm: frozen TF makes scoring corpus-independent (S4.3.3, M9).

ER-053 shipped the `tf_lookup` schema, :func:`~er.matching.tf.materialize_tf_lookup`
and :func:`~er.matching.tf.register_tf`. What it could not ship is evidence that the
frozen values *work*: that registering them actually makes `match_probability` a pure
function of `(model_version, tf_snapshot_id, rec_a_key, rec_b_key, rec_a_content_hash,
rec_b_content_hash)` — INV-SCORE — rather than of the corpus that happened to be
registered. Nothing in the schema can show that, because the failure it prevents is a
number changing when nothing about the pair did.

So this module grows the corpus and asserts the numbers do not move. `base_10` is
scored, then ~1,000 generated records are ingested into the **same namespace** and the
whole thing is scored again at the **same** `model_version` and `tf_snapshot_id`. Every
pair present in both runs must carry an exactly equal probability.

Three properties of that comparison are load-bearing:

* **`==`, with no tolerance.** A tolerance would hide precisely the drift this exists
  to detect: an unfrozen TF adjustment moves a probability in the fifth decimal, not
  the first, and `math.isclose` would call that stable.
* **The corpus grows by ingestion, not by rebuild.** The second run re-scores the
  `base_10` pairs through the S4.3.4 MERGE key, so the comparison is against values
  *persisted by the first run* rather than against a second in-memory result. A
  rebuild would compare two fresh computations and could not see a MERGE that
  appended instead of updating.
* **No probability literal is committed.** The two arms are compared against each
  other. A committed constant would turn a model refit into a test failure with
  nothing wrong, and would make the test pass on a pipeline that returned that
  constant for everything.

The refusal arm is the other half of D4. Splink computes term frequency from whatever
corpus is registered at predict time, so a run that reaches `predict()` with an
incomplete `tf_lookup` does not fail — it silently scores against the corpus at hand.
:func:`~er.matching.tf.assert_tf_lookup_complete` is the preflight that makes that
unreachable, and `test_missing_tf_lookup_exits_3` drives it through the real console
script so the exit code, the `error_class` and the no-write claim are all asserted
where an operator would meet them.

The `Dbt` harness and the delivery helper are duplicated from
`tests/integration/test_full_match.py` rather than imported, for the reason that module
states: a test module importing another test module makes a node id's dependencies
invisible to whoever reads it.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from collections.abc import Iterator
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from types import ModuleType
from typing import Any, Final

import duckdb
import pytest
from helpers.model import fixture_settings, load_fixture_model
from helpers.scenario import Scenario, load_scenario
from splink.internals.linker_components.table_management import LinkerTableManagement
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.ids import canonicalize_pair
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.api import assert_no_splink_relations_in_lake
from er.matching.evidence import TF_ADJUSTMENT_PREFIX
from er.matching.full import MATCH_SCORES_RELATION, MODE_FULL, score_full
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.matching.tf import TF_LOOKUP_RELATION, tf_columns
from er.obs.counters import DECLARED_COUNTERS, StageCounters
from er.obs.runctx import StageRun

REPO_ROOT: Final = Path(__file__).resolve().parents[2]


def _import(module: str) -> ModuleType:
    """Import a `fixtures/generator` module, whose parent is not on the path.

    `fixtures/` is committed data rather than a distribution, so nothing installs it
    and `pythonpath` carries only `tests`. A bare `sys.path` statement followed by an
    import is an E402 this ticket may not suppress, so the indirection is the same one
    `tests/unit/generator/test_emit.py` uses.
    """
    entry = str(REPO_ROOT / "fixtures")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(f"generator.{module}", fromlist=["_"])


emit = _import("emit")
personas_module = _import("personas")

#: The S8.2 fixture the compared pairs come from, and its only phase.
SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

#: The two dbt selectors `er standardize` runs (S4.2).
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.{MATCH_SCORES_RELATION}"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
TF_LOOKUP: Final = f"{SCHEMA_QUALIFIER}.{TF_LOOKUP_RELATION}"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

MATCH_STAGE: Final = "match"

#: The grown corpus. `base_10` holds 23 records, so ~1,000 more is the ~40x of the
#: ticket: large enough that any unfrozen term frequency moves visibly (a value's
#: relative frequency over 23 records and over 1,023 are not close), small enough that
#: the PR path can afford to standardize and score it.
GENERATED_PERSONAS: Final = 400
GENERATED_RECORDS: Final = 1000

#: S4.7's `error_class` for a refused precondition, and S4.0's exit code for one.
PRECONDITION_CLASS: Final = "precondition"


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


def generated_delivery(root: Path, cfg: Config) -> tuple[Path, int]:
    """Emit ~`GENERATED_RECORDS` records and lay them out as a drop folder.

    `emit_corpus` writes `<out_dir>/<source>.csv` beside a `truth.csv`, because a
    benchmark drops a base corpus straight into `storage.drop_dir`. `er ingest --source
    <s> --path <root>` reads `<root>/<s>/`, so the files are moved into that shape here
    and `truth.csv` is left behind — the pipeline never sees ground truth (S8.2.1).

    The generated ids are `C00000001`-style, nine characters wide, and `base_10`'s are
    `C001`; no generated record can collide with a fixture one, which is what lets both
    live in one namespace and makes the second run a re-score rather than a replacement.

    Returns:
        The drop-folder root, and how many records were written across all sources.
    """
    staging = root / "_emitted"
    spec = emit.CorpusSpec(
        seed=cfg.generator.seed, personas=GENERATED_PERSONAS, records=GENERATED_RECORDS
    )
    personas = personas_module.generate_personas(spec.seed, spec.personas, spec.household_rate)
    written = emit.emit_corpus(spec, personas, staging, config=cfg)

    drop = root / "drop_generated"
    records = 0
    for path in written:
        if path.name == emit.TRUTH_FILENAME:
            continue
        source = path.stem
        directory = drop / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
        # -1 for the header row: the count is what AC1's "~40x" is measured against.
        records += max(0, len(path.read_text(encoding="utf-8").splitlines()) - 1)
    return drop, records


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def score_rows(connection: duckdb.DuckDBPyConnection) -> dict[tuple[str, str], dict[str, Any]]:
    """Every `match_scores` row, keyed by its canonical pair."""
    columns = (
        "rec_a_key",
        "rec_b_key",
        "match_probability",
        "model_version",
        "tf_snapshot_id",
        "rec_a_content_hash",
        "rec_b_content_hash",
        "is_active",
        "evidence",
    )
    rows = connection.execute(f"SELECT {', '.join(columns)} FROM {MATCH_SCORES}").fetchall()
    keyed: dict[tuple[str, str], dict[str, Any]] = {}
    for row in rows:
        record = dict(zip(columns, row, strict=True))
        keyed[canonicalize_pair(str(record["rec_a_key"]), str(record["rec_b_key"]))] = record
    return keyed


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
class TableManagementSpy:
    """Every TF-related `table_management` call one scoring run made (AC4).

    A counting spy rather than an inspection of the result, because AC4's claim is a
    *negative*: `compute_tf_table` was not called. Nothing in `match_scores` can
    distinguish a frozen adjustment from one Splink computed over the same corpus, so
    the only place the difference exists is the call.
    """

    registered: list[str] = field(default_factory=list)
    computed: list[str] = field(default_factory=list)


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


@dataclass
class Harness:
    """One namespace with `base_10` standardized in it, plus the tools to grow it."""

    connection: duckdb.DuckDBPyConnection
    dbt: Dbt
    cfg: Config
    tmp_path: Path

    def grow(self) -> int:
        """Ingest the generated corpus into this namespace and re-standardize."""
        drop, records = generated_delivery(self.tmp_path, self.cfg)
        for source in sorted(path.name for path in drop.iterdir() if path.is_dir()):
            result = run_er("ingest", "--source", source, "--path", str(drop))
            assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
        self.dbt.standardize()
        return records

    def score(self, model: tuple[str, str], *, run_id: str | None = None) -> StageRun:
        """One in-process `er match --mode full` at ``model``."""
        model_version, tf_snapshot_id = model
        stage_run = StageRun(
            run_id=run_id if run_id is not None else str(ULID()),
            stage=MATCH_STAGE,
            seq=1,
            started_at=datetime.now(UTC),
            counters=StageCounters(DECLARED_COUNTERS[MATCH_STAGE]),
        )
        score_full(
            self.connection,
            self.cfg,
            stage_run,
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            settings=fixture_settings(),
        )
        return stage_run


@pytest.fixture
def harness(
    base_10: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[Harness]:
    """`base_10` ingested and standardized on this test's fresh namespace.

    The teardown restores the connection's default schema, for the reason
    `test_full_match.py` states: `splink_api` repoints the shared session handle.
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
        yield Harness(connection=initialised_lake, dbt=dbt, cfg=cfg, tmp_path=tmp_path)
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


@pytest.fixture
def model(harness: Harness) -> tuple[str, str]:
    """The committed model installed into this lake: `(model_version, tf_snapshot_id)`."""
    model_version, tf_snapshot_id, _ = load_fixture_model(harness.connection)
    return model_version, tf_snapshot_id


@pytest.fixture
def table_management_spy(monkeypatch: pytest.MonkeyPatch) -> TableManagementSpy:
    """Count `register_term_frequency_lookup` and `compute_tf_table` calls (AC4)."""
    spy = TableManagementSpy()
    register = LinkerTableManagement.register_term_frequency_lookup
    compute = LinkerTableManagement.compute_tf_table

    def recording_register(
        self: LinkerTableManagement, input_data: Any, col_name: str, overwrite: bool = False
    ) -> Any:
        spy.registered.append(col_name)
        return register(self, input_data, col_name, overwrite)

    def recording_compute(self: LinkerTableManagement, column_name: str, *args: Any) -> Any:
        spy.computed.append(column_name)
        return compute(self, column_name, *args)

    monkeypatch.setattr(LinkerTableManagement, "register_term_frequency_lookup", recording_register)
    monkeypatch.setattr(LinkerTableManagement, "compute_tf_table", recording_compute)
    return spy


def test_score_is_corpus_size_independent(harness: Harness, model: tuple[str, str]) -> None:
    """AC1/AC2/AC5/AC6: a ~40x corpus does not move a single probability.

    INV-SCORE says `match_probability` is a pure function of the model key and the two
    endpoints. The corpus is not in that tuple, so growing it must change nothing about
    the pairs already scored — and it is exactly what would change everything if term
    frequency were computed at predict time instead of registered from the frozen
    snapshot (D4).
    """
    before = harness.score(model)
    first = score_rows(harness.connection)
    assert first, "the first run scored nothing; every comparison below would be vacuous"

    small_corpus = int(scalar(harness.connection, f"SELECT count(*) FROM {STD_RECORDS}"))
    added = harness.grow()
    grown_corpus = int(scalar(harness.connection, f"SELECT count(*) FROM {STD_RECORDS}"))
    assert grown_corpus > small_corpus * 10, (
        f"the corpus grew from {small_corpus} to {grown_corpus} ({added} records "
        "delivered); a growth this small would not move an unfrozen term frequency "
        "enough for this test to detect one"
    )

    after_run = harness.score(model)
    second = score_rows(harness.connection)
    assert after_run.run_id != before.run_id

    compared = sorted(set(first) & set(second))
    assert compared, "no pair survived the re-score; the MERGE replaced rather than updated"
    for pair in compared:
        # `==` on the DOUBLE, no tolerance: an unfrozen adjustment moves the fifth
        # decimal, and any tolerance wide enough to be "safe" would swallow it.
        assert first[pair]["match_probability"] == second[pair]["match_probability"], (
            f"{pair} scored {first[pair]['match_probability']!r} over {small_corpus} "
            f"records and {second[pair]['match_probability']!r} over {grown_corpus}; "
            "INV-SCORE says the corpus is not an input (S4.3.3, D4)"
        )
        # AC2: the endpoints' own hashes and the active flag survive the re-score.
        assert second[pair]["rec_a_content_hash"] == first[pair]["rec_a_content_hash"]
        assert second[pair]["rec_b_content_hash"] == first[pair]["rec_b_content_hash"]
        assert second[pair]["is_active"] is True

    # AC1's rider: the compared set must include a pair whose winning level on a
    # `tf: true` column is the exact one. Equal non-NULL values on that column IS the
    # exact level, and the frozen adjustment for it is in the evidence — so this is the
    # case where an unfrozen TF would actually have changed the number.
    values = {
        str(record_key): row
        for record_key, *row in harness.connection.execute(
            f"SELECT record_key, {', '.join(tf_columns(harness.cfg))} FROM {STD_RECORDS}"
        ).fetchall()
    }
    columns = tf_columns(harness.cfg)
    exact_on_tf = [
        (pair, column)
        for pair in compared
        for index, column in enumerate(columns)
        if values[pair[0]][index] is not None and values[pair[0]][index] == values[pair[1]][index]
    ]
    assert exact_on_tf, (
        f"no compared pair matches exactly on any of {list(columns)}, so none of them "
        "exercises a term-frequency adjustment and the invariant is untested"
    )
    pair, column = exact_on_tf[0]
    evidence = json.loads(str(second[pair]["evidence"]))
    assert f"{TF_ADJUSTMENT_PREFIX}{column}" in evidence, (
        f"{pair} matches exactly on {column} ({values[pair[0]][columns.index(column)]!r}) "
        f"but its evidence carries no {TF_ADJUSTMENT_PREFIX}{column} (D4, S4.3.3)"
    )

    # AC2: the MERGE key is unique, so the second run updated rather than appended.
    duplicates = scalar(
        harness.connection,
        f"SELECT count(*) - count(DISTINCT (rec_a_key, rec_b_key, model_version, "
        f"tf_snapshot_id)) FROM {MATCH_SCORES}",
    )
    assert duplicates == 0, f"{duplicates} rows share the S4.3.4 MERGE key"

    # AC5: one TF snapshot across everything, and it is the active registry row's.
    active = scalar(
        harness.connection,
        f"SELECT tf_snapshot_id FROM {MODEL_REGISTRY} WHERE status = 'active'",
    )
    assert (
        scalar(harness.connection, f"SELECT count(DISTINCT tf_snapshot_id) FROM {MATCH_SCORES}")
        == 1
    )
    assert (
        scalar(
            harness.connection, f"SELECT count(*) FROM {MATCH_SCORES} WHERE tf_snapshot_id IS NULL"
        )
        == 0
    )
    for record in second.values():
        assert record["tf_snapshot_id"] == active
        assert record["model_version"] == model[0]

    # AC6.
    assert_no_splink_relations_in_lake(harness.connection)


def test_missing_tf_lookup_exits_3(
    harness: Harness, model: tuple[str, str], cfg: Config, object_store: ObjectStore
) -> None:
    """AC3: one column's frozen rows deleted -> exit 3, named column, zero writes.

    Driven through the real console script, because the claim is about what an operator
    meets: the exit code, the `error_class` and the absence of a partial write. The
    settings are published to the object store first for the reason
    `test_full_match.py` gives — `er match` loads them through
    `model_registry.params_path`, and the committed fixture is a file on disk.
    """
    model_version, tf_snapshot_id = model
    settings = fixture_settings()
    published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
    object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
    harness.connection.execute(
        f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
        [published, model_version],
    )

    columns = tf_columns(cfg)
    assert columns, "the config declares no tf: true column; AC3 would be vacuous"
    victim = columns[0]
    deleted = scalar(
        harness.connection,
        f"SELECT count(*) FROM {TF_LOOKUP} WHERE model_version = ? AND tf_snapshot_id = ? "
        f"AND column_name = ?",
        model_version,
        tf_snapshot_id,
        victim,
    )
    assert deleted > 0, f"{victim} had no frozen rows to delete; the arm would be vacuous"
    harness.connection.execute(
        f"DELETE FROM {TF_LOOKUP} WHERE model_version = ? AND tf_snapshot_id = ? "
        f"AND column_name = ?",
        [model_version, tf_snapshot_id, victim],
    )

    before = scalar(harness.connection, f"SELECT count(*) FROM {MATCH_SCORES}")
    run_id = str(ULID())
    refused = run_er("match", "--mode", MODE_FULL, "--run-id", run_id)
    assert refused.returncode == int(ExitCode.PRECONDITION), (
        f"exit {refused.returncode}, expected {int(ExitCode.PRECONDITION)}; a missing "
        f"frozen lookup is a precondition (S4.0, S4.7), never a scoring failure.\n"
        f"{refused.stdout}\n{refused.stderr}"
    )

    row = harness.connection.execute(
        f"SELECT error_class, error_detail FROM {RUN_STAGES} WHERE run_id = ? AND stage = ?",
        [run_id, MATCH_STAGE],
    ).fetchall()
    assert len(row) == 1, f"{len(row)} run_stages rows for the refused run; S5.2 writes one"
    error_class, error_detail = row[0]
    assert str(error_class) == PRECONDITION_CLASS
    assert victim in str(error_detail), (
        f"error_detail does not name the missing column {victim!r}; the operator action "
        f"is to re-materialize that column. Got: {error_detail!r}"
    )

    after = scalar(harness.connection, f"SELECT count(*) FROM {MATCH_SCORES}")
    assert after == before, (
        f"the refused run added {after - before} match_scores rows; the guard must run "
        "before any write so a refusal is a true no-write"
    )


def test_tf_registered_per_column_and_compute_tf_table_never_called(
    harness: Harness, model: tuple[str, str], table_management_spy: TableManagementSpy
) -> None:
    """AC4/AC6: one registration per `tf: true` column, and Splink never computes TF.

    The positive and the negative are one claim: every TF column is registered from the
    frozen snapshot, and none is computed from the corpus at hand. Registering a subset
    would be worse than registering none — Splink would compute the rest, and the
    resulting probability would be neither the frozen value nor a reproducible one.
    """
    harness.score(model)

    expected = list(tf_columns(harness.cfg))
    assert sorted(table_management_spy.registered) == sorted(expected), (
        f"registered {table_management_spy.registered}, expected one call per tf: true "
        f"column {expected} (S4.3.1)"
    )
    assert len(table_management_spy.registered) == len(expected), (
        f"{len(table_management_spy.registered)} registrations for {len(expected)} "
        f"columns; a column registered twice joins the corpus twice: "
        f"{table_management_spy.registered}"
    )
    assert table_management_spy.computed == [], (
        f"Splink computed term frequency for {table_management_spy.computed}; D4 freezes "
        "TF at training time and a scoring run may only register it (S4.3.3)"
    )

    assert_no_splink_relations_in_lake(harness.connection)
