"""`base_10` staged through the three S4.2 models (S4.2, S5, S5.0, S8.1, S8.3).

Everything here is asserted against a real `dbt build` on the namespaced lake, not
against compiled SQL, because the claims S4.2 makes about `stg_<source>` are claims
about rows: that the three relations hold exactly what `raw_records` delivered, in
the shape S5 declares, and that running the build twice over one `ingest_batch_id`
appends nothing.

Four properties, one test each:

* **shape** -- the row counts, the S5 column list read back out of the catalog, and
  the contract that makes a drift from it fail `dbt build` rather than land.
* **standardization** -- `name_variants`' element-0 symmetry guarantee, and the two
  `date_format`s meeting on one `birth_date`.
* **idempotence** -- the not-in-distinct-batch predicate (M16).
* **the logical key** -- `(source_system, source_record_id, content_hash)` is a dbt
  test or it is nothing (S5.0).

dbt is invoked through `er.dbt_runner.run_dbt`, the one place the pipeline spawns
it, with the `--vars` payload `render_dbt_vars` builds from `configs/test.yaml`. So
what these tests exercise is the models reading their column mapping out of the S6
document (S4.2) rather than out of `dbt_project.yml`'s parse-time fallbacks. The
session's DuckDB connection is detached for the length of every invocation and
re-attached after it: S4.0b says no Python connection spans a dbt subprocess.

`er standardize` is deliberately not the entry point. In this milestone it is still
a no-op stub -- ER-043 wires the stage to `run_dbt` -- and a test that drove the stub
would assert nothing about the models this ticket ships.
"""

from __future__ import annotations

import csv
import os
import shutil
import subprocess
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import date, datetime
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
from er.lake.columns import VOLATILE_COLUMNS
from er.lake.dbt_sources import KEYS_TAG
from er.lake.ddl import live_columns
from er.lake.ducklake import attach_statements, connect, detach
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER

#: The S8.2 fixture this suite stages, and its only phase.
SCENARIO_NAME = "base_10"
PHASE = "base"

#: The three sources `configs/test.yaml` declares, in `sources:` order.
SOURCES: tuple[str, ...] = ("crm", "billing", "webforms")

#: AC1's literal: `base_10` delivers 23 records across the three CSVs.
TOTAL_ROWS = 23

#: What `--select staging` names, and what `dbt-selector:staging` promises callers.
STAGING_SELECTOR = "staging"

#: The sibling selector, needed only as setup for `tag:keys` — see
#: `test_duplicate_key_fails_tag_keys`. This module asserts nothing about it.
INTERMEDIATE_SELECTOR = "intermediate"

#: T-KEY-1's selector, from the tag ER-021 puts on every logical-key test.
KEYS_SELECTOR = f"tag:{KEYS_TAG}"

#: The fixture bookkeeping column S8.2 carries in every input CSV. It is mapped by no
#: `sources.<name>.columns` entry, so it must reach no staged relation.
PERSONA_COLUMN = "persona_id"

#: How the catalog views spell S5's one non-scalar type (S5.0: `LIST(VARCHAR)` is
#: supported and `ARRAY` is not). `ddl.py` holds the same mapping for the same reason.
CATALOG_SPELLING: Mapping[str, str] = {"LIST(VARCHAR)": "VARCHAR[]"}

#: dbt writes a model's rendered SQL here; `<project>/target/compiled/<project name>/`.
COMPILED_ROOT = Path(DBT_PROJECT_DIR) / "target" / "compiled" / "er" / "models" / "staging"

#: S4.2's predicate as it survives rendering: `{{ this }}` becomes the relation, and
#: everything to the left of it is the literal the spec writes.
COMPILED_PREDICATE = "ingest_batch_id not in (select distinct ingest_batch_id from"

#: The extra `crm` delivery of AC4's year-only arm. `base_10` carries no year-only DOB
#: -- every persona has a full date -- so the row that proves a year-only value yields
#: NULL has to be delivered rather than found.
YEAR_ONLY_ID = "C900"
YEAR_ONLY_HEADER: tuple[str, ...] = (
    "crm_id",
    "first_name",
    "last_name",
    "email_address",
    "phone",
    "street_address",
    "city",
    "state",
    "zip",
    "dob",
    "last_modified",
)
YEAR_ONLY_ROW: tuple[str, ...] = (
    YEAR_ONLY_ID,
    "Yearonly",
    "Person",
    "yearonly@example.com",
    "(415) 555-0999",
    "1 Year Way",
    "San Francisco",
    "CA",
    "94110",
    "1985",
    "2024-05-01 10:00:00",
)


def relation_for(source: str) -> str:
    """The `stg_<source>` relation S4.2 gives one source (S5)."""
    return f"stg_{source}"


def stable_columns(relation: str) -> tuple[str, ...]:
    """`relation`'s S5 columns minus VOLATILE_COLUMNS (S5.0).

    Read off the registry rather than transcribed, so a column added to S5 is
    compared by the next run instead of being silently outside the comparison.
    """
    return tuple(
        column.name for column in REGISTRY[relation].columns if column.name not in VOLATILE_COLUMNS
    )


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

    The image ships no `dbt/dbt_packages` (`.dockerignore`), and the staging
    contract's logical-key test is `dbt_utils.unique_combination_of_columns`. Run as
    a plain subprocess: `deps` touches no warehouse, so it is the one invocation that
    is not a stage and does not go through `run_dbt`.
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
def staged(base_10: Scenario, dbt: Dbt, tmp_path: Path) -> Dbt:
    """`base_10` ingested and built through `--select staging`.

    The seed runs first and is not part of the selection: `name_variants` reaches
    `nickname_variants` with `ref()` (S4.2), and `--select staging` names the three
    models and nothing upstream of them.
    """
    delivery = deliver(base_10, tmp_path / "drop")
    for source in base_10.inputs_for(PHASE):
        result = run_er("ingest", "--source", source, "--path", str(delivery))
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    dbt("seed")
    dbt("build", select=STAGING_SELECTOR)
    return dbt


def test_base_10_stages_23_rows_with_contract_shape(
    initialised_lake: duckdb.DuckDBPyConnection, base_10: Scenario, staged: Dbt, tmp_path: Path
) -> None:
    """AC1/AC2: the delivered rows, the S5 shape, and the contract that holds it."""
    with connect() as connection:
        counts = {source: row_count(connection, relation_for(source)) for source in SOURCES}
        shapes = {
            relation_for(source): dict(live_columns(connection, relation_for(source)))
            for source in SOURCES
        }

    # AC1: one staged row per delivered record, and the three together are base_10.
    assert counts == {source: len(delivered_rows(base_10, source)) for source in SOURCES}
    assert sum(counts.values()) == TOTAL_ROWS

    for source in SOURCES:
        relation = relation_for(source)
        declared = [
            (column.name, CATALOG_SPELLING.get(column.type, column.type))
            for column in REGISTRY[relation].columns
        ]
        # AC2: names, ORDER and types, against the S5 registry rather than against a
        # list retyped here -- S5 is the review-time authority for a dbt-owned
        # relation and the contract is what makes it one (S5.0).
        assert list(shapes[relation].items()) == declared, relation
        # AC1: `persona_id` is fixture bookkeeping. No `columns:` entry maps it, so it
        # cannot reach a staged relation however the models are rendered.
        assert PERSONA_COLUMN in delivered_rows(base_10, source)[0]
        assert PERSONA_COLUMN not in shapes[relation]

    # AC2's negative arm, on a scratch copy so the project under test is never
    # edited: retyping one contracted column makes `dbt build` fail, which is the
    # whole reason S5.0 asks for `contract: {enforced: true}`.
    scratch = tmp_path / "scratch-dbt"
    shutil.copytree(Path(DBT_PROJECT_DIR), scratch, ignore=shutil.ignore_patterns("target", "logs"))
    contract = scratch / "models" / "staging" / "schema.yml"
    retyped = contract.read_text(encoding="utf-8").replace(
        "      - name: birth_date\n        data_type: DATE\n",
        "      - name: birth_date\n        data_type: VARCHAR\n",
        1,
    )
    assert retyped != contract.read_text(encoding="utf-8"), "the scratch edit changed nothing"
    contract.write_text(retyped, encoding="utf-8")

    with pytest.raises(StageFailure):
        staged("build", select=STAGING_SELECTOR, project_dir=str(scratch))

    output = staged.last_output()
    assert "contract" in output.lower(), output
    assert "birth_date" in output, output


def test_name_variants_symmetry_and_date_formats(
    base_10: Scenario, staged: Dbt, tmp_path: Path
) -> None:
    """AC3/AC4: element-0 symmetry, and two `date_format`s meeting on one DATE."""
    sources = config().sources

    with connect() as connection:
        variants = {
            source: query(
                connection,
                f"SELECT source_record_id, given_name, name_variants "
                f"FROM {SCHEMA_QUALIFIER}.{relation_for(source)} ORDER BY source_record_id",
            )
            for source in SOURCES
        }
        births = {
            source: dict(
                query(
                    connection,
                    f"SELECT source_record_id, birth_date "
                    f"FROM {SCHEMA_QUALIFIER}.{relation_for(source)}",
                )
            )
            for source in SOURCES
        }

    # AC3: NOT NULL on every row, and element 0 is the record's own normalized
    # `given_name` -- the S4.2 guarantee `variant_match` orientation-independence
    # rests on. DuckDB lists are 1-based, so element 0 is `[1]`.
    staged_rows = sum(len(rows) for rows in variants.values())
    assert staged_rows == TOTAL_ROWS
    for source, rows in variants.items():
        for record_id, given_name, name_variants in rows:
            assert name_variants is not None, (
                f"{source}/{record_id}: name_variants is NOT NULL (S5)"
            )
            # Every base_10 record carries a given name, so the empty-array arm the
            # macro renders for a NULL name is not reachable here and is not asserted
            # around: element 0 IS the record's own normalized `given_name`.
            assert given_name is not None, f"{source}/{record_id}: base_10 delivers a given name"
            assert list(name_variants)[0] == given_name, (
                f"{source}/{record_id}: {name_variants!r} does not start with {given_name!r}"
            )

    # AC4: every staged `birth_date` is the delivered value read through that
    # source's own `date_format` (S6) -- which is the claim, since `%m/%d/%Y` and
    # `%Y-%m-%d` describe the same day differently.
    by_persona: dict[str, dict[str, date | None]] = {}
    for source in SOURCES:
        spec = sources[source]
        for row in delivered_rows(base_10, source):
            delivered = row[spec.columns["birth_date"]]
            expected = datetime.strptime(delivered, spec.date_format).date()
            record_id = row[spec.record_id_column]
            assert births[source][record_id] == expected, f"{source}/{record_id}: {delivered}"
            by_persona.setdefault(row[PERSONA_COLUMN], {})[source] = births[source][record_id]

    # AC4, stated as S8.2 states it: a persona delivered to billing under `%m/%d/%Y`
    # and to crm under `%Y-%m-%d` is one person with one date of birth. Webforms is
    # excluded here and only here: `base_10`'s drifted-DOB trap deliberately delivers
    # P1 a day late, and folding it in would assert the trap away.
    shared = {
        persona: dates for persona, dates in by_persona.items() if {"crm", "billing"} <= set(dates)
    }
    assert shared, "base_10 delivers personas to both crm and billing"
    for persona, dates in sorted(shared.items()):
        assert dates["crm"] == dates["billing"], f"{persona}: {dates}"

    # AC4's second half: a year-only value is not usable matching evidence, so it
    # parses to NULL rather than to January 1st (S4.2).
    directory = tmp_path / "year-only" / "crm"
    directory.mkdir(parents=True)
    with (directory / "crm.csv").open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(YEAR_ONLY_HEADER)
        writer.writerow(YEAR_ONLY_ROW)
    result = run_er("ingest", "--source", "crm", "--path", str(tmp_path / "year-only"))
    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    staged("build", select=STAGING_SELECTOR)

    with connect() as connection:
        rows = query(
            connection,
            f"SELECT given_name, birth_date FROM {SCHEMA_QUALIFIER}.stg_crm "
            f"WHERE source_record_id = ?",
            YEAR_ONLY_ID,
        )

    # The row IS staged -- the NULL is the parse refusing a year, not a record that
    # never arrived, and the two would be indistinguishable without this assertion.
    assert len(rows) == 1, rows
    (given_name, birth_date) = rows[0]
    assert given_name == "yearonly"
    assert birth_date is None, "a year-only DOB yields NULL, never a fabricated 1 January"


def test_restandardize_appends_no_rows(staged: Dbt) -> None:
    """AC5: a second build over the same batch appends nothing (M16)."""

    def snapshot(connection: duckdb.DuckDBPyConnection) -> dict[str, list[Any]]:
        return {
            relation_for(source): query(
                connection,
                f"SELECT {', '.join(stable_columns(relation_for(source)))} "
                f"FROM {SCHEMA_QUALIFIER}.{relation_for(source)} "
                f"ORDER BY source_record_id, content_hash",
            )
            for source in SOURCES
        }

    with connect() as connection:
        before = snapshot(connection)
        counts_before = {source: row_count(connection, relation_for(source)) for source in SOURCES}

    staged("build", select=STAGING_SELECTOR)

    with connect() as connection:
        after = snapshot(connection)
        counts_after = {source: row_count(connection, relation_for(source)) for source in SOURCES}

    assert counts_after == counts_before
    assert sum(counts_after.values()) == TOTAL_ROWS
    # Not merely the counts: every non-VOLATILE value of every surviving row is
    # unchanged, which is what "logically unchanged" means in the S4 preamble.
    assert after == before

    # The predicate is what did it. The compiled artifact is read from the run that
    # just happened, so this asserts the SQL dbt actually executed -- on the first
    # build `is_incremental()` is false and the predicate is legitimately absent.
    for source in SOURCES:
        relation = relation_for(source)
        compiled = (COMPILED_ROOT / f"{relation}.sql").read_text(encoding="utf-8")
        assert COMPILED_PREDICATE in compiled, relation
        assert relation in compiled.split(COMPILED_PREDICATE, 1)[1], (
            f"{relation}: the predicate must read the model's own relation ({{ this }})"
        )


def test_duplicate_key_fails_tag_keys(
    initialised_lake: duckdb.DuckDBPyConnection, staged: Dbt
) -> None:
    """AC7: the `stg_*` logical key is a dbt test or it is nothing (S5.0)."""
    # The `keys` tag covers `int_std_records` from ER-043 onwards, and a selector is
    # not an arm: `tag:keys` names every dbt-owned logical key there is, so the
    # relations behind them have to exist before it can report on any of them. The
    # intermediate build is setup for the selector, not a claim this test makes --
    # what it asserts is still the `stg_*` key and nothing else.
    staged("build", select=INTERMEDIATE_SELECTOR)
    staged("test", select=KEYS_SELECTOR)

    # DuckLake enforces no uniqueness, so this lands silently -- which is precisely
    # why `(source_system, source_record_id, content_hash)` has to be a test.
    initialised_lake.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.stg_crm "
        f"SELECT * FROM {SCHEMA_QUALIFIER}.stg_crm ORDER BY source_record_id LIMIT 1"
    )

    with pytest.raises(StageFailure):
        staged("test", select=KEYS_SELECTOR)

    output = staged.last_output()
    assert "stg_crm" in output, output
