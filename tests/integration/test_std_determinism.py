"""T-STD-1: standardization determinism as a content hash (S8.3, S5.0, S4.2, M7).

M7 is the reason this suite exists in this shape. "Byte-identical `std_records`" is
not implementable — Parquet output is not reproducible, and `int_std_records` carries
the `VOLATILE_COLUMNS` ingest stamps, which differ between two runs that produced
identical data — so T-STD-1 asserts a **content hash over an explicit stable column
list** instead, and asserts nothing whatever about the data files. That second half is
a real clause of the spec and is enforced here by a guard over this module's own
source, because "a comparison that was never written" is otherwise invisible.

Two arms:

* **stability** — standardize `base_10` twice and compare the 23 `(record_key,
  std_hash)` pairs. `ingest_batch_id` and `ingested_at` are free to differ; they are
  outside `STD_HASH_COLUMNS` by construction, which the test asserts rather than
  assumes.
* **sensitivity** — re-deliver one record with a corrected `given_name`, and exactly
  that record's hash moves. A digest that never changed would satisfy the first arm
  perfectly, so the two arms are only meaningful together.

`er standardize` is not the entry point: it is still ER-014's `NoOpStage` and returns
`10` without touching a model, so this suite drives dbt through `er.dbt_runner.run_dbt`
with the `--vars` payload `render_dbt_vars` builds — the same two selections the stage
will run once it is wired. `tests/integration/scenarios/test_int_std_records.py` made
that choice first and records the reasoning; the small `Dbt` harness is duplicated from
it rather than imported, because a test module importing another test module hides a
node id's dependencies from whoever reads it.
"""

from __future__ import annotations

import ast
import csv
import os
import subprocess
from dataclasses import dataclass
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
from er.errors import ExitCode
from er.lake.columns import VOLATILE_COLUMNS
from er.lake.ducklake import attach_statements, connect, detach
from er.lake.hashing import STD_HASH_COLUMNS, table_content_hash
from er.lake.model import SCHEMA_QUALIFIER

#: The S8.2 fixture T-STD-1 runs on, and its only phase.
SCENARIO_NAME = "base_10"
PHASE = "base"

#: The relation under test and the two selections `er standardize` runs (S4.2).
MODEL = "int_std_records"
STAGING_SELECTOR = "staging"
INTERMEDIATE_SELECTOR = "intermediate"

#: `base_10` is 23 records over 10 personas (S8.2), all of them current.
TOTAL_ROWS = 23

#: The source whose record AC5 corrects, and the corrected value. A given name absent
#: from `dbt/seeds/nickname_variants.csv`, so the edit moves `given_name` itself rather
#: than only rearranging the `name_variants` array around an unchanged head.
CORRECTED_SOURCE = "crm"
CORRECTED_GIVEN_NAME = "Bartholomew"

#: T-STD-1's second sentence, as something a test can fail on: this module compares no
#: data file, and reads the object store through neither of `ObjectStore`'s two read
#: methods (ER-015). Each token is spelled in two halves so the guard does not match on
#: its own definition -- otherwise it would be a rule that trips on the line declaring
#: it. The scan is over *code* (see :func:`code_text`), so the prose above is free to
#: explain the rule using the very words the rule forbids.
FORBIDDEN_TOKENS: tuple[str, ...] = ("par" + "quet", "list_" + "prefix", "get_" + "bytes")


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


def code_text(path: Path) -> str:
    """Everything ``path`` *executes*: identifiers and non-docstring literals, lowercased.

    Comments and docstrings are excluded because they carry the explanation of the very
    rule the guard enforces -- a textual grep over the file would make documenting
    T-STD-1's "no Parquet comparison" clause impossible to state in the module that
    obeys it. A SQL string or an imported name is code and stays in scope.
    """
    tree = ast.parse(path.read_text(encoding="utf-8"))
    documented = (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)
    docstrings = {
        ast.get_docstring(node, clean=False)
        for node in ast.walk(tree)
        if isinstance(node, documented)
    }
    pieces: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            if node.value not in docstrings:
                pieces.append(node.value)
        elif isinstance(node, ast.Name):
            pieces.append(node.id)
        elif isinstance(node, ast.Attribute):
            pieces.append(node.attr)
        elif isinstance(node, ast.alias):
            pieces.append(node.name)
        elif isinstance(node, ast.FunctionDef | ast.ClassDef):
            pieces.append(node.name)
    return "\n".join(pieces).lower()


def std_hashes(connection: duckdb.DuckDBPyConnection) -> dict[str, str]:
    """The T-STD-1 `(record_key, std_hash)` map of the current `int_std_records`.

    The projection is `STD_HASH_COLUMNS` and nothing else, so a volatile column cannot
    reach the preimage even if a future edit added one to the relation.
    """
    projection = ", ".join(STD_HASH_COLUMNS)
    rows = query(
        connection,
        f"SELECT {projection} FROM {SCHEMA_QUALIFIER}.{MODEL} ORDER BY record_key",
    )
    return {
        str(row[0]): table_content_hash(dict(zip(STD_HASH_COLUMNS, row, strict=True)))
        for row in rows
    }


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


def deliver_correction(scenario: Scenario, root: Path, record_id: str, given_name: str) -> Path:
    """The same `crm` delivery with one record's given name corrected (AC5).

    A whole re-delivery rather than a hand-written single-row file, because that is
    what a corrected extract looks like: S4.1's anti-join appends a version for the one
    row whose `content_hash` moved and nothing for the other seven, so the blast radius
    the second arm measures is produced by the pipeline rather than staged by the test.

    The column names come from S6 rather than from the CSV header, which is where the
    mapping is declared.
    """
    spec = config().sources[CORRECTED_SOURCE]
    source_path = scenario.inputs_for(PHASE)[CORRECTED_SOURCE]
    with source_path.open(encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle)
        fieldnames = reader.fieldnames or []
        rows = list(reader)

    corrected = [row for row in rows if row[spec.record_id_column] == record_id]
    assert len(corrected) == 1, f"{record_id}: exactly one delivered row carries this id"
    assert corrected[0][spec.columns["given_name"]] != given_name, "the correction changes nothing"
    corrected[0][spec.columns["given_name"]] = given_name

    directory = root / CORRECTED_SOURCE
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / source_path.name).open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)
    return root


@dataclass(frozen=True)
class Dbt:
    """dbt as a stage invokes it: real `--vars`, no connection spanning it (S4.0b)."""

    connection: duckdb.DuckDBPyConnection
    artifacts: Path

    def __call__(self, command: str, select: str | None = None) -> DbtResult:
        return run_dbt(
            command,
            select=select,
            vars=render_dbt_vars(config(), str(ULID())),
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


@pytest.fixture
def base_10() -> Scenario:
    """The S8.2 fixture, opened through ER-028's loader."""
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (PHASE,)
    return scenario


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored.

    The image ships no `dbt/dbt_packages` (`.dockerignore`), and the `int_std_records`
    key tests are `dbt_utils` macros. A plain subprocess: `deps` touches no warehouse,
    so it is the one invocation that is not a stage.
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
    """`base_10` ingested and standardized once.

    The seed runs first and is in neither selection: `name_variants` reaches
    `nickname_variants` through `ref()` (S4.2), and no path selector names a seed.
    """
    delivery = deliver(base_10, tmp_path / "drop")
    for source in base_10.inputs_for(PHASE):
        result = run_er("ingest", "--source", source, "--path", str(delivery))
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    dbt("seed")
    dbt.standardize()
    return dbt


def test_std_content_hash_stable(standardized: Dbt) -> None:
    """T-STD-1 (AC4): two standardize runs, one `(record_key, std_hash)` map."""
    with connect() as connection:
        first = std_hashes(connection)

    assert len(first) == TOTAL_ROWS

    standardized.standardize()

    with connect() as connection:
        second = std_hashes(connection)

    # Byte-identical, keys and values: `==` on two `dict[str, str]` is exactly the
    # 23-pair comparison T-STD-1 asks for, and it fails on a lost record as well as on
    # a moved hash.
    assert second == first

    # "`ingest_batch_id` and `ingested_at` are allowed to differ" is not a promise the
    # test keeps by looking away -- those columns cannot reach the preimage, because
    # the projection is `STD_HASH_COLUMNS` and S5.0's frozen set is disjoint from it.
    assert set(STD_HASH_COLUMNS) & VOLATILE_COLUMNS == set()
    assert {"ingest_batch_id", "ingested_at"} <= VOLATILE_COLUMNS

    # T-STD-1's second sentence: byte identity of the Parquet files is NOT asserted,
    # and neither is anything else about them. The claim is about what this module does
    # not do, so it is checked against the module's own source.
    executed = code_text(Path(__file__))
    named = [token for token in FORBIDDEN_TOKENS if token in executed]
    assert named == [], f"T-STD-1 asserts no data-file comparison, but this module names {named}"


def test_single_record_change_moves_one_hash(
    base_10: Scenario, standardized: Dbt, tmp_path: Path
) -> None:
    """AC5: a corrected `given_name` moves exactly one hash and leaves 22 alone."""
    with connect() as connection:
        before = std_hashes(connection)
        record_id = str(
            query(
                connection,
                f"SELECT source_record_id FROM {SCHEMA_QUALIFIER}.{MODEL} "
                f"WHERE source_system = ? ORDER BY source_record_id LIMIT 1",
                CORRECTED_SOURCE,
            )[0][0]
        )

    corrected_key = f"{CORRECTED_SOURCE}:{record_id}"
    assert corrected_key in before

    delivery = deliver_correction(base_10, tmp_path / "correction", record_id, CORRECTED_GIVEN_NAME)
    result = run_er("ingest", "--source", CORRECTED_SOURCE, "--path", str(delivery))
    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    standardized.standardize()

    with connect() as connection:
        after = std_hashes(connection)
        given_names = dict(
            query(connection, f"SELECT record_key, given_name FROM {SCHEMA_QUALIFIER}.{MODEL}")
        )

    # The correction supersedes a version rather than adding a record (S4.2).
    assert len(after) == TOTAL_ROWS
    assert set(after) == set(before)

    moved = sorted(key for key in before if after[key] != before[key])
    assert moved == [corrected_key]
    assert {key: after[key] for key in after if key != corrected_key} == {
        key: before[key] for key in before if key != corrected_key
    }

    # Non-vacuous: the standardized value really is the corrected one, so the moved
    # hash is attributable to the edit and not to some incidental churn.
    assert given_names[corrected_key] == CORRECTED_GIVEN_NAME.lower()
