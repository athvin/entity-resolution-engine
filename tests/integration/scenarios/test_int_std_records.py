"""`base_10` standardized into `int_std_records` (S4.2, S5, S5.0, S4.1, S4.1.1, S8.1).

`int_std_records` is the relation Splink's `unique_id_column_name` points at and the
one every later stage reads, so what this suite asserts is *current state*: exactly
one row per `(source_system, source_record_id)`, carrying the `raw_records` version
with the greatest `ingested_at`, and carrying none at all for a key whose winning
version is a tombstone. Those are claims about rows, which is why every one of them
is asserted against a real `dbt build` on the namespaced lake (S8.1) rather than
against compiled SQL.

Seven properties, one test each:

* **shape** -- 23 current rows out of `base_10`, `record_key` materialized on every
  one of them, and `dbt build --select int_std_records` green under its contract.
* **supersession** -- the greatest-`ingested_at` rule and its `ingest_batch_id DESC`
  tiebreak (M15). The tiebreak is `DESC` because the ULID is time-ordered, so `ASC`
  would let the *older* batch win, and a fixture where the two happen to agree would
  never notice.
* **tombstones and resurrection** -- exclusion is by the WINNING version's hash, not
  by any version having it, which is exactly what lets a re-delivered key come back
  with no special case (S4.1.1).
* **the contract** -- the S5 column list, order and types read back out of the
  catalog, and the enforcement that makes a drift fail `dbt build` (S5.0).
* **the logical keys** -- DuckLake enforces neither, so `record_key` uniqueness, the
  one-current-row rule and the `':'` ban are dbt tests or they are nothing.
* **validity flags** -- `email_valid` and `phone_valid` travel together, because the
  `validated` survivorship rule needs an input for both attributes (S6.1 V4).
* **row stability** -- a second standardize over an unchanged corpus is a no-op, and
  the resolved model configuration is S4.2's `delete+insert` (M16).

Two deliberate choices about how the models are driven.

dbt is invoked through `er.dbt_runner.run_dbt`, the one place the pipeline spawns
it, with the `--vars` payload `render_dbt_vars` builds from `configs/test.yaml`, and
the session's connection is detached for the length of every invocation (S4.0b).
`er standardize` is NOT the entry point: it is still the `NoOpStage` ER-014 gave it
and returns `10` without touching a model, so driving it would assert nothing about
the relation this ticket ships. `tests/integration/scenarios/test_staging.py` made
the same choice for the same reason, and AC7's "re-run" is therefore a second build
over the same selection -- which is what the stage will do once it is wired.

The `Dbt` harness and the small query helpers are duplicated from that module rather
than imported. A test module importing another test module makes a node id's
dependencies invisible to whoever reads it, and `tests/integration/conftest.py`
belongs to another ticket; `tests/integration/test_keys.py` records the same
reasoning.
"""

from __future__ import annotations

import csv
import json
import os
import shutil
import subprocess
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

import duckdb
import pytest
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import (
    DBT_PROFILES_DIR,
    DBT_PROJECT_DIR,
    DbtResult,
    render_dbt_vars,
    run_dbt,
)
from er.errors import ExitCode, StageFailure
from er.ingest.landing import TOMBSTONE_CONTENT_HASH, TOMBSTONE_PAYLOAD
from er.lake.columns import VOLATILE_COLUMNS
from er.lake.dbt_sources import KEYS_TAG
from er.lake.ddl import live_columns
from er.lake.ducklake import attach_statements, connect, detach
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER

#: The S8.2 fixture this suite standardizes, and its only phase.
SCENARIO_NAME = "base_10"
PHASE = "base"

#: The three sources `configs/test.yaml` declares, in `sources:` order.
SOURCES: tuple[str, ...] = ("crm", "billing", "webforms")

#: The relation under test, and the `dbt-selector:intermediate` that names it.
MODEL = "int_std_records"
INTERMEDIATE_SELECTOR = "intermediate"
STAGING_SELECTOR = "staging"

#: AC1's literal: `base_10` is 23 records over 10 personas (S8.2), and every one of
#: them is current, because the fixture delivers one version per key.
TOTAL_ROWS = 23

#: T-KEY-1b's selector, from the tag ER-021 puts on every logical-key test.
KEYS_SELECTOR = f"tag:{KEYS_TAG}"

#: How the catalog views spell S5's one non-scalar type (S5.0: `LIST(VARCHAR)` is
#: supported and `ARRAY` is not). `ddl.py` holds the same mapping for the same reason.
CATALOG_SPELLING: Mapping[str, str] = {"LIST(VARCHAR)": "VARCHAR[]"}

#: dbt's resolved node config, which is where AC7's "the compiled model declares"
#: actually lives: `{{ config() }}` is stripped from the compiled SQL, and the
#: manifest is the artifact that records what dbt resolved from the model AND from
#: `dbt_project.yml`'s roots together.
MANIFEST = Path(DBT_PROJECT_DIR) / "target" / "manifest.json"
MODEL_NODE = f"model.er.{MODEL}"

#: The model's rendered SQL; `<project>/target/compiled/<project name>/`. It is where
#: the S4.1.1 sentinel becomes a literal, the Jinja source carrying only the name of
#: the variable that produces it.
COMPILED_ROOT = Path(DBT_PROJECT_DIR) / "target" / "compiled" / "er" / "models" / "intermediate"
COMPILED_MODEL = COMPILED_ROOT / f"{MODEL}.sql"

#: S4.2's incremental configuration for the `int_*` family, as dbt resolves it.
EXPECTED_MODEL_CONFIG: Mapping[str, Any] = {
    "materialized": "incremental",
    "incremental_strategy": "delete+insert",
    "unique_key": ["source_system", "source_record_id"],
    "on_schema_change": "append_new_columns",
}

#: The instants the crafted `raw_records` versions carry. Later than anything
#: `er ingest` stamped in this session, because "current" is defined by
#: `ingested_at` and a version that is not the greatest proves nothing (S4.2).
TIE_INSTANT = datetime(2030, 1, 1)
LATER_INSTANT = datetime(2030, 1, 2)

#: The content hashes the crafted versions carry. Any 64 hex characters that are not
#: the S4.1.1 sentinel will do; distinct letters make a failure legible.
TIE_LOSER_HASH = "a" * 64
TIE_WINNER_HASH = "b" * 64
LATER_HASH = "c" * 64
RESURRECTED_HASH = "d" * 64

#: AC5's `source_record_id`, staged directly so that the `':'` ban is exercised on a
#: relation the pipeline built rather than on one the test hand-wrote.
COLON_RECORD_ID = "BAD:1"


def config() -> Config:
    """The validated S6 document this session runs against."""
    return load_config(Path(os.environ["ER_CONFIG"]))


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def query(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: Any) -> list[Any]:
    return connection.execute(sql, list(parameters)).fetchall()


def row_count(connection: duckdb.DuckDBPyConnection, relation: str) -> int:
    (count,) = query(connection, f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{relation}")[0]
    return int(count)


def stable_columns() -> tuple[str, ...]:
    """`int_std_records`' S5 columns minus VOLATILE_COLUMNS (S5.0).

    Read off the registry rather than transcribed, so a column added to S5 is
    compared by the next run instead of being silently outside the comparison.
    """
    return tuple(
        column.name for column in REGISTRY[MODEL].columns if column.name not in VOLATILE_COLUMNS
    )


def deliver(scenario: Scenario, root: Path) -> Path:
    """Materialise the scenario's phase as the drop-folder root `er ingest --path` reads.

    A scenario stores a phase as `<phase>/<source>.csv` (S8.2.1) while S4.1 reads
    `<path>/<source>/`, so the files are copied rather than pointed at.
    """
    for source, path in scenario.inputs_for(PHASE).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def delivered_rows(scenario: Scenario, source: str) -> list[dict[str, str]]:
    """One source CSV, as the dicts the adapter would read (S4.1)."""
    with scenario.inputs_for(PHASE)[source].open(encoding="utf-8", newline="") as handle:
        return list(csv.DictReader(handle))


def append_raw_version(
    connection: duckdb.DuckDBPyConnection,
    source_system: str,
    source_record_id: str,
    *,
    content_hash: str,
    ingest_batch_id: str,
    ingested_at: datetime,
    is_deleted: bool = False,
) -> None:
    """Append one `raw_records` version for an existing key (S4.1: append-only, D7).

    A content version copies the key's existing `payload` so the staged projection
    stays a real record; a tombstone carries S4.1.1's sentinel hash, the JSON-null
    payload ER-032 writes, and `deleted_at` equal to the batch's own instant.

    Written straight to `raw_records` rather than through `er ingest`, because what
    S4.2 selects between is *versions*, and a delivery cannot express "the same key,
    two versions, the same `ingested_at`, two batches" -- which is the tiebreak.
    """
    columns = (
        "(source_system, source_record_id, payload, content_hash, "
        "is_deleted, deleted_at, ingest_batch_id, ingested_at)"
    )
    if is_deleted:
        connection.execute(
            f"INSERT INTO {SCHEMA_QUALIFIER}.raw_records {columns} "
            f"VALUES (?, ?, CAST(? AS JSON), ?, TRUE, ?, ?, ?)",
            [
                source_system,
                source_record_id,
                TOMBSTONE_PAYLOAD,
                content_hash,
                ingested_at,
                ingest_batch_id,
                ingested_at,
            ],
        )
        return
    connection.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.raw_records {columns} "
        f"SELECT source_system, source_record_id, payload, ?, FALSE, NULL, ?, ? "
        f"FROM {SCHEMA_QUALIFIER}.raw_records "
        f"WHERE source_system = ? AND source_record_id = ? "
        f"ORDER BY ingested_at DESC LIMIT 1",
        [
            content_hash,
            ingest_batch_id,
            ingested_at,
            source_system,
            source_record_id,
        ],
    )


def ordered_batch_ids() -> tuple[str, str]:
    """Two `ingest_batch_id`s, smaller first. Minted, then sorted, never assumed.

    The tiebreak is lexical on the id, and asserting it against two ULIDs whose
    order was taken on faith would assert the mint order instead.
    """
    minted = sorted({str(ULID()) for _ in range(2)})
    assert len(minted) == 2, "two mints, two distinct ULIDs"
    return minted[0], minted[1]


def first_crm_record_id(connection: duckdb.DuckDBPyConnection) -> str:
    """The lowest `crm` key `int_std_records` currently holds.

    Chosen by the relation rather than named as a literal, so the supersession tests
    below stay runnable against a fixture edit that renumbers `base_10`.
    """
    rows = query(
        connection,
        f"SELECT source_record_id FROM {SCHEMA_QUALIFIER}.{MODEL} "
        f"WHERE source_system = 'crm' ORDER BY source_record_id LIMIT 1",
    )
    assert rows, "base_10 delivers crm records"
    return str(rows[0][0])


def current_hash(connection: duckdb.DuckDBPyConnection, source: str, record_id: str) -> list[Any]:
    """Every `content_hash` `int_std_records` currently holds for one key."""
    return [
        row[0]
        for row in query(
            connection,
            f"SELECT content_hash FROM {SCHEMA_QUALIFIER}.{MODEL} "
            f"WHERE source_system = ? AND source_record_id = ?",
            source,
            record_id,
        )
    ]


@dataclass(frozen=True)
class Dbt:
    """dbt as a stage invokes it: real `--vars`, no connection spanning it (S4.0b)."""

    connection: duckdb.DuckDBPyConnection
    artifacts: Path

    def __call__(
        self,
        command: str,
        select: str | None = None,
        *,
        project_dir: str = DBT_PROJECT_DIR,
    ) -> DbtResult:
        return run_dbt(
            command,
            select=select,
            vars=render_dbt_vars(config(), str(ULID())),
            target="lake",
            close_conn=lambda: detach(self.connection),
            reopen_conn=self._reattach,
            project_dir=project_dir,
            profiles_dir=str(Path(project_dir) / "profiles"),
            artifacts_dir=self.artifacts,
        )

    def standardize(self) -> None:
        """The selection `er standardize` runs: staging, then intermediate (S4.2)."""
        self("build", select=STAGING_SELECTOR)
        self("build", select=INTERMEDIATE_SELECTOR)

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)

    def last_output(self) -> str:
        """The captured stdout+stderr of the most recent invocation.

        `run_dbt` persists it on the failure path too, which is what lets a negative
        arm assert on WHAT dbt objected to rather than merely that it objected.
        """
        logs = sorted((self.artifacts / "dbt").glob("*.log"), key=lambda path: path.stat().st_mtime)
        assert logs, "run_dbt writes a log for every invocation"
        return logs[-1].read_text(encoding="utf-8")


@pytest.fixture
def base_10() -> Scenario:
    """The S8.2 fixture, opened through ER-028's loader."""
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (PHASE,)
    return scenario


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored.

    The image ships no `dbt/dbt_packages` (`.dockerignore`), and the one-current-row
    key is a `dbt_utils.unique_combination_of_columns`. Run as a plain subprocess:
    `deps` touches no warehouse, so it is the one invocation that is not a stage.
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
def dbt(dbt_packages: None, initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path) -> Dbt:
    """dbt bound to this test's lake handle and its own artifacts root."""
    return Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts")


@pytest.fixture
def standardized(base_10: Scenario, dbt: Dbt, tmp_path: Path) -> Dbt:
    """`base_10` ingested and standardized through staging and intermediate.

    The seed runs first and is not part of either selection: `name_variants` reaches
    `nickname_variants` with `ref()` (S4.2), and neither path selector names it.
    """
    delivery = deliver(base_10, tmp_path / "drop")
    for source in base_10.inputs_for(PHASE):
        result = run_er("ingest", "--source", source, "--path", str(delivery))
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    dbt("seed")
    dbt.standardize()
    return dbt


def test_base_10_yields_23_current_rows_with_record_key(standardized: Dbt) -> None:
    """AC1: 23 current rows, `record_key` on every one, and a green contracted build."""
    with connect() as connection:
        total = row_count(connection, MODEL)
        mismatched = query(
            connection,
            f"SELECT record_key, source_system, source_record_id "
            f"FROM {SCHEMA_QUALIFIER}.{MODEL} "
            f"WHERE record_key is distinct from source_system || ':' || source_record_id",
        )
        by_source = dict(
            query(
                connection,
                f"SELECT source_system, count(*) FROM {SCHEMA_QUALIFIER}.{MODEL} "
                f"GROUP BY source_system",
            )
        )

    assert total == TOTAL_ROWS
    # Not merely the total: every source contributed, so a union that silently lost
    # a branch cannot pass by having the other two hold 23 rows between them.
    assert set(by_source) == set(SOURCES)
    assert sum(by_source.values()) == TOTAL_ROWS
    assert mismatched == [], mismatched

    # AC1's third clause. It is a real assertion and not a repetition of the fixture:
    # `--select int_std_records` builds the model alone, under the enforced contract
    # `dbt_project.yml` puts on every dbt-owned relation (S5.0).
    standardized("build", select=MODEL)

    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    assert manifest["nodes"][MODEL_NODE]["config"]["contract"]["enforced"] is True


def test_greatest_ingested_at_wins_with_batch_id_tiebreak(
    initialised_lake: duckdb.DuckDBPyConnection, standardized: Dbt
) -> None:
    """AC2: the greatest `ingested_at` is current, and `ingest_batch_id DESC` breaks a tie."""
    with connect() as connection:
        record_id = first_crm_record_id(connection)

    # The tie arm first, because it is the one an `ASC` tiebreak gets wrong: two
    # versions of one key, the SAME `ingested_at`, two batches (S4.2, M15).
    losing_batch, winning_batch = ordered_batch_ids()
    append_raw_version(
        initialised_lake,
        "crm",
        record_id,
        content_hash=TIE_LOSER_HASH,
        ingest_batch_id=losing_batch,
        ingested_at=TIE_INSTANT,
    )
    append_raw_version(
        initialised_lake,
        "crm",
        record_id,
        content_hash=TIE_WINNER_HASH,
        ingest_batch_id=winning_batch,
        ingested_at=TIE_INSTANT,
    )
    standardized.standardize()

    with connect() as connection:
        after_tie = current_hash(connection, "crm", record_id)
        total_after_tie = row_count(connection, MODEL)

    assert after_tie == [TIE_WINNER_HASH], (
        f"{record_id}: the lexically greater ingest_batch_id ({winning_batch}) must win a "
        f"tie on ingested_at; ASC would have taken {losing_batch}"
    )
    # A second version of a known key adds no record, so the corpus is still 23.
    assert total_after_tie == TOTAL_ROWS

    # The greatest-`ingested_at` arm: a strictly later version wins outright, and the
    # batch id it carries is irrelevant to that.
    append_raw_version(
        initialised_lake,
        "crm",
        record_id,
        content_hash=LATER_HASH,
        ingest_batch_id=str(ULID()),
        ingested_at=LATER_INSTANT,
    )
    standardized.standardize()

    with connect() as connection:
        after_later = current_hash(connection, "crm", record_id)
        total_after_later = row_count(connection, MODEL)

    assert after_later == [LATER_HASH]
    assert total_after_later == TOTAL_ROWS


def test_tombstoned_key_excluded_and_resurrected_key_returns(
    initialised_lake: duckdb.DuckDBPyConnection, standardized: Dbt
) -> None:
    """AC3: a tombstoned key holds no row, and a re-delivered one comes back."""
    # The sentinel is S4.1.1's, and the model spells it as a SQL literal because a
    # dbt model cannot import the Python constant that wrote the rows it reads. This
    # is the assertion that binds the two spellings together, and it reads the
    # COMPILED artifact of the build the fixture just ran, so what it compares
    # against is the predicate dbt executed rather than the Jinja that produced it.
    compiled = COMPILED_MODEL.read_text(encoding="utf-8")
    assert f"content_hash <> '{TOMBSTONE_CONTENT_HASH}'" in compiled, compiled

    with connect() as connection:
        record_id = first_crm_record_id(connection)

    append_raw_version(
        initialised_lake,
        "crm",
        record_id,
        content_hash=TOMBSTONE_CONTENT_HASH,
        ingest_batch_id=str(ULID()),
        ingested_at=TIE_INSTANT,
        is_deleted=True,
    )
    standardized.standardize()

    with connect() as connection:
        after_tombstone = current_hash(connection, "crm", record_id)
        total_after_tombstone = row_count(connection, MODEL)
        # `tombstones_excluded` as a stage counter reads exactly this: the keys whose
        # winning staged version carries the sentinel (S4.2's counter list).
        (tombstones_excluded,) = query(
            connection,
            f"SELECT count(*) FROM ("
            f"  SELECT content_hash FROM {SCHEMA_QUALIFIER}.stg_crm"
            f"  QUALIFY row_number() OVER ("
            f"    PARTITION BY source_system, source_record_id"
            f"    ORDER BY ingested_at DESC, ingest_batch_id DESC) = 1"
            f") WHERE content_hash = ?",
            TOMBSTONE_CONTENT_HASH,
        )[0]

    assert after_tombstone == [], (
        f"{record_id}: the winning version is a tombstone, so S4.2 excludes the key entirely"
    )
    assert total_after_tombstone == TOTAL_ROWS - 1
    assert tombstones_excluded == 1
    assert total_after_tombstone == TOTAL_ROWS - tombstones_excluded

    # Resurrection (S4.1.1): an ordinary content version whose `ingested_at` is
    # greater than the tombstone's. The supersession rule alone makes the record live
    # again -- there is no resurrection branch in the model to get wrong.
    append_raw_version(
        initialised_lake,
        "crm",
        record_id,
        content_hash=RESURRECTED_HASH,
        ingest_batch_id=str(ULID()),
        ingested_at=LATER_INSTANT,
    )
    standardized.standardize()

    with connect() as connection:
        after_resurrect = current_hash(connection, "crm", record_id)
        total_after_resurrect = row_count(connection, MODEL)

    assert after_resurrect == [RESURRECTED_HASH]
    assert total_after_resurrect == TOTAL_ROWS


def test_contract_matches_s5_column_list(standardized: Dbt, tmp_path: Path) -> None:
    """AC4: the relation's columns, order and types are S5's, and the contract holds them."""
    with connect() as connection:
        shape = dict(live_columns(connection, MODEL))

    declared = [
        (column.name, CATALOG_SPELLING.get(column.type, column.type))
        for column in REGISTRY[MODEL].columns
    ]
    # Against the S5 registry rather than against a list retyped here: S5 is the
    # review-time authority for a dbt-owned relation, and the contract is what makes
    # it one (S5.0).
    assert list(shape.items()) == declared

    # The three columns AC4 names explicitly, because each is a column a plausible
    # implementation drops: the non-scalar type, and the two validity flags that are
    # NOT golden_records columns and so look removable from a mart's point of view.
    assert shape["name_variants"] == CATALOG_SPELLING["LIST(VARCHAR)"]
    assert shape["email_valid"] == "BOOLEAN"
    assert shape["phone_valid"] == "BOOLEAN"

    # The negative arm, on a scratch copy so the project under test is never edited:
    # retyping one contracted column makes `dbt build` fail, which is the whole
    # reason S5.0 asks for `contract: {enforced: true}`.
    scratch = tmp_path / "scratch-dbt"
    shutil.copytree(Path(DBT_PROJECT_DIR), scratch, ignore=shutil.ignore_patterns("target", "logs"))
    contract = scratch / "models" / "intermediate" / "schema.yml"
    retyped = contract.read_text(encoding="utf-8").replace(
        "      - name: birth_date\n        data_type: DATE\n",
        "      - name: birth_date\n        data_type: VARCHAR\n",
        1,
    )
    assert retyped != contract.read_text(encoding="utf-8"), "the scratch edit changed nothing"
    contract.write_text(retyped, encoding="utf-8")

    with pytest.raises(StageFailure):
        standardized("build", select=INTERMEDIATE_SELECTOR, project_dir=str(scratch))

    output = standardized.last_output()
    assert "contract" in output.lower(), output
    assert "birth_date" in output, output


def test_colon_and_duplicate_key_fail_tag_keys(
    initialised_lake: duckdb.DuckDBPyConnection, standardized: Dbt
) -> None:
    """AC5: both `int_std_records` keys are dbt tests or they are nothing (S5.0)."""
    standardized("test", select=KEYS_SELECTOR)

    # A second current row for one `(source_system, source_record_id)`. DuckLake
    # enforces no uniqueness, so this lands silently -- which is precisely why the
    # one-current-row rule has to be a test (S5.0, S4.2).
    initialised_lake.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.{MODEL} "
        f"SELECT * FROM {SCHEMA_QUALIFIER}.{MODEL} ORDER BY record_key LIMIT 1"
    )

    with pytest.raises(StageFailure):
        standardized("test", select=KEYS_SELECTOR)
    assert MODEL in standardized.last_output(), standardized.last_output()

    # Rebuilding restores the invariant, which is what makes the next arm's failure
    # attributable to the colon and not to the duplicate still standing: the
    # `delete+insert` strategy removes every row of a touched key before re-inserting
    # its winner (S4.2). `run` and not `build`, here and below, because `build` would
    # run the very tests this test means to interrogate separately -- the negative arm
    # has to be asserted against `tag:keys`, which is the selector AC5 names.
    standardized("run", select=INTERMEDIATE_SELECTOR)
    standardized("test", select=KEYS_SELECTOR)

    # A staged row whose `source_record_id` carries the banned `':'` (S5.0, D6),
    # inserted into staging so that it reaches `int_std_records` the way a real one
    # would. `* REPLACE` keeps every other staged value, so the id is the only thing
    # dbt can be objecting to.
    initialised_lake.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.stg_crm "
        f"SELECT * REPLACE (? AS source_record_id) "
        f"FROM {SCHEMA_QUALIFIER}.stg_crm ORDER BY source_record_id LIMIT 1",
        [COLON_RECORD_ID],
    )
    standardized("run", select=INTERMEDIATE_SELECTOR)

    with connect() as connection:
        staged_colon = query(
            connection,
            f"SELECT record_key FROM {SCHEMA_QUALIFIER}.{MODEL} WHERE source_record_id = ?",
            COLON_RECORD_ID,
        )

    # The row IS current -- the model does not filter it out, and the ban is a test.
    assert staged_colon == [(f"crm:{COLON_RECORD_ID}",)], staged_colon

    with pytest.raises(StageFailure):
        standardized("test", select=KEYS_SELECTOR)
    assert MODEL in standardized.last_output(), standardized.last_output()


def test_phone_and_email_valid_populated(base_10: Scenario, standardized: Dbt) -> None:
    """AC6: both validity flags are populated, and the placeholder emails are NULL."""
    sources = config().sources
    placeholders = set(config().standardization.email_placeholders)

    with connect() as connection:
        unflagged_email = query(
            connection,
            f"SELECT record_key FROM {SCHEMA_QUALIFIER}.{MODEL} "
            f"WHERE email is not null AND email_valid is null",
        )
        unflagged_phone = query(
            connection,
            f"SELECT record_key FROM {SCHEMA_QUALIFIER}.{MODEL} "
            f"WHERE phone_e164 is not null AND phone_valid is null",
        )
        (valid_phones,) = query(
            connection,
            f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{MODEL} WHERE phone_valid",
        )[0]
        emails = dict(
            query(
                connection,
                f"SELECT record_key, email FROM {SCHEMA_QUALIFIER}.{MODEL}",
            )
        )

    assert unflagged_email == [], unflagged_email
    assert unflagged_phone == [], unflagged_phone
    # Non-vacuous: `base_10`'s drifted-phone trap normalizes three spellings to one
    # E.164 number, so a corpus where every flag is NULL cannot pass the two above by
    # having no phone at all.
    assert valid_phones > 0

    # The placeholder arm, with the addresses read from S6 rather than retyped: S4.2
    # gives placeholder nulling to `email_norm` and to no other macro, so a record
    # delivered with one carries a NULL `email` here.
    placeholder_keys = [
        f"{source}:{row[sources[source].record_id_column]}"
        for source in SOURCES
        for row in delivered_rows(base_10, source)
        if row[sources[source].columns["email"]] in placeholders
    ]
    assert len(placeholder_keys) == 2, placeholder_keys
    for key in placeholder_keys:
        assert emails[key] is None, f"{key}: a placeholder address is nulled by email_norm (S4.2)"


def test_restandardize_is_row_stable(standardized: Dbt) -> None:
    """AC7: a second standardize is a no-op, and the resolved config is S4.2's (M16)."""

    def snapshot(connection: duckdb.DuckDBPyConnection) -> list[Any]:
        return query(
            connection,
            f"SELECT {', '.join(stable_columns())} FROM {SCHEMA_QUALIFIER}.{MODEL} "
            f"ORDER BY record_key",
        )

    with connect() as connection:
        before = snapshot(connection)
        count_before = row_count(connection, MODEL)

    standardized.standardize()

    with connect() as connection:
        after = snapshot(connection)
        count_after = row_count(connection, MODEL)

    assert count_before == TOTAL_ROWS
    assert count_after == TOTAL_ROWS
    # Not merely the counts: every non-VOLATILE value of every row is unchanged,
    # which is what "logically unchanged" means in the S4 preamble.
    assert after == before

    # AC7's second half, read from the manifest rather than from the compiled SQL:
    # `{{ config() }}` is stripped from the compiled artifact, and the manifest is
    # where dbt records what it resolved from the model and from `dbt_project.yml`
    # together -- which is the value the materialization actually ran under.
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    resolved = manifest["nodes"][MODEL_NODE]["config"]
    for key, value in EXPECTED_MODEL_CONFIG.items():
        assert resolved[key] == value, f"{key}: {resolved[key]!r} != {value!r}"
