"""Frozen TF against a real lake: what `materialize_tf_lookup` writes, and why (S4.3.3).

D4's claim is arithmetic — a frozen `tf_value` is the value's relative frequency in the
corpus that was current when the snapshot was taken — so it is asserted against a real
`int_std_records` built from `base_10` by a real `er ingest` plus `dbt build`, not
against a hand-made frame. Recomputing the frequency in the test from the same relation
is not circular: the materializer computes it in one SQL statement over all TF columns
at once, and the check counts one column's values in Python.

Three properties here are about the lake rather than about the arithmetic, and each is
a way frozen TF fails silently:

* `tf_lookup`'s logical key `(model_version, tf_snapshot_id, column_name, value)` is
  unenforced — DuckLake supports no `UNIQUE` (S5.0) — so a second materialization under
  one key must replace rather than append. Appending would give every value two
  frequencies and double the corpus on the registration join.
* Registering frozen TF must not put anything in the lake. Splink registers what it is
  given as a relation, and S4.0b's guarantee is that no `__splink__` relation and no
  snapshot follows from a scoring run.
* A key with no rows must stop the run (S4.0 exit `3`). The alternative is Splink
  computing TF from the corpus at hand, which is exactly the INV-SCORE break D4 exists
  to prevent, and which no assertion downstream would catch.

The ingest-plus-dbt harness is duplicated from
`tests/integration/scenarios/test_base_10_std.py` rather than imported, for its reason:
a test module importing another test module makes a node id's dependencies invisible to
whoever reads the failure.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.scenario import Scenario, load_scenario
from splink import Linker
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode, exit_code_for
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.api import assert_no_splink_relations_in_lake, splink_api
from er.matching.model import build_settings
from er.matching.tf import (
    MissingTfLookupError,
    materialize_tf_lookup,
    new_tf_snapshot_id,
    register_tf,
    tf_columns,
)

#: The S8.2 fixture this suite freezes TF over, and its only phase.
SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

#: The two dbt selectors `er standardize` runs (S4.2).
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
TF_LOOKUP: Final = f"{SCHEMA_QUALIFIER}.tf_lookup"

#: The `comparisons` keys S6 marks `tf: true`, and the three it does not. Spelled out
#: rather than read from the config, so AC1 is a claim about the document as well as
#: about the materializer.
TF: Final = ("given_name", "family_name", "email")
NOT_TF: Final = ("phone_e164", "birth_date", "addr_postal")

#: A registry version this suite owns. `model_version` allocation is ER-054's; nothing
#: here reads `model_registry`, so the key is simply a value both halves agree on.
MODEL_VERSION: Final = "v0001"

#: S4.3.3's tolerance for AC2. `tf_value` is a `DOUBLE` computed by one division in
#: DuckDB and by another in Python, so they agree to rather more than this.
TF_TOLERANCE: Final = 1e-12

#: The local frame the S4.0b-sanctioned way: materialized into the in-memory database
#: and referenced by a BARE name, because Splink resolves an input table's columns
#: through `information_schema.columns WHERE table_name = '<name>'` and a qualified
#: name matches no row there.
FRAME: Final = "er_053_frame"

#: The columns `configs/test.yaml` references — its four blocking expressions and its
#: six comparisons — plus `record_key` as `unique_id_column_name`.
FRAME_COLUMNS: Final = (
    "record_key",
    "given_name",
    "family_name",
    "name_variants",
    "email",
    "phone_e164",
    "birth_date",
    "addr_postal",
)


def config() -> Config:
    """The validated S6 document this session runs against."""
    return load_config(Path(os.environ["ER_CONFIG"]))


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def deliver(scenario: Scenario, root: Path) -> Path:
    """Materialise the phase as the drop-folder root `er ingest --path` reads (S4.1)."""
    for source, path in scenario.inputs_for(PHASE).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


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


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return load_config(Path(os.environ["ER_CONFIG"]))


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
    tmp_path: Path,
) -> duckdb.DuckDBPyConnection:
    """`base_10` ingested and standardized on this test's fresh namespace.

    The seed runs first and is part of neither selection: `name_variants` reaches
    `nickname_variants` through `ref()` (S4.2).
    """
    delivery = deliver(base_10, tmp_path / "drop")
    for source in base_10.inputs_for(PHASE):
        result = run_er("ingest", "--source", source, "--path", str(delivery))
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts")
    dbt("seed")
    dbt.standardize()
    return initialised_lake


@pytest.fixture
def frozen(standardized: duckdb.DuckDBPyConnection, cfg: Config) -> Iterator[str]:
    """One materialized TF snapshot over the standardized corpus; yields its id."""
    tf_snapshot_id = new_tf_snapshot_id()
    written = materialize_tf_lookup(standardized, cfg, MODEL_VERSION, tf_snapshot_id)
    assert written > 0, "base_10 has non-NULL values in every TF column"
    yield tf_snapshot_id


def corpus_values(connection: duckdb.DuckDBPyConnection, column: str) -> list[str]:
    """Every non-NULL value of one column of `int_std_records`, as text.

    As text because `tf_lookup.value` is `VARCHAR` (S5) and the materializer casts on
    the way in; comparing the frozen key against a differently-rendered value would
    fail for a reason that has nothing to do with the frequency.
    """
    rows = connection.execute(
        f"SELECT CAST({column} AS VARCHAR) FROM {STD_RECORDS} WHERE {column} IS NOT NULL"
    ).fetchall()
    return [str(value) for (value,) in rows]


def test_materialize_writes_one_row_per_value(
    frozen: str, standardized: duckdb.DuckDBPyConnection, cfg: Config
) -> None:
    """AC1: exactly the `tf: true` columns, one row per distinct non-NULL value."""
    assert tf_columns(cfg) == TF

    materialized = [
        str(name)
        for (name,) in standardized.execute(
            f"SELECT DISTINCT column_name FROM {TF_LOOKUP} "
            f"WHERE model_version = ? AND tf_snapshot_id = ? ORDER BY column_name",
            [MODEL_VERSION, frozen],
        ).fetchall()
    ]
    assert materialized == sorted(TF)

    # The negative half: a column without `tf: true` contributing rows would be
    # invisible until a scoring run registered TF for a level S4.3.1 forbids it on.
    for column in NOT_TF:
        assert column in cfg.comparisons, f"{column} is not a comparison in this document"
        assert (
            scalar(
                standardized,
                f"SELECT count(*) FROM {TF_LOOKUP} WHERE column_name = ?",
                column,
            )
            == 0
        )

    for column in TF:
        distinct_values = len(set(corpus_values(standardized, column)))
        rows = scalar(
            standardized,
            f"SELECT count(*) FROM {TF_LOOKUP} WHERE model_version = ? "
            f"AND tf_snapshot_id = ? AND column_name = ?",
            MODEL_VERSION,
            frozen,
            column,
        )
        assert rows == distinct_values, f"{column}: {rows} frozen rows, {distinct_values} values"


def test_tf_values_equal_corpus_relative_frequency(
    frozen: str, standardized: duckdb.DuckDBPyConnection
) -> None:
    """AC2: every `tf_value` is the value's share of the column's non-NULL values."""
    for column in TF:
        values = corpus_values(standardized, column)
        assert values, f"{column} is NULL on every record; the check would be vacuous"

        frozen_rows = {
            str(value): float(tf_value)
            for value, tf_value in standardized.execute(
                f"SELECT value, tf_value FROM {TF_LOOKUP} "
                f"WHERE model_version = ? AND tf_snapshot_id = ? AND column_name = ?",
                [MODEL_VERSION, frozen, column],
            ).fetchall()
        }
        assert set(frozen_rows) == set(values), f"{column}: frozen values differ from the corpus"

        for value, tf_value in frozen_rows.items():
            expected = values.count(value) / len(values)
            assert abs(tf_value - expected) < TF_TOLERANCE, (
                f"{column}={value!r}: frozen {tf_value}, corpus {expected}"
            )

        # The frequencies of one column partition the non-NULL corpus, so they sum to
        # 1: a per-value check alone would pass on a denominator that counted NULLs.
        assert abs(sum(frozen_rows[value] for value in set(values)) - 1.0) < TF_TOLERANCE


def test_rematerialize_does_not_duplicate_the_key(
    frozen: str, standardized: duckdb.DuckDBPyConnection, cfg: Config
) -> None:
    """AC3: a second materialization under one key replaces rather than appends.

    DuckLake enforces no `UNIQUE` (S5.0), so nothing but the writer stops the second
    write from doubling every row — and a doubled `tf_lookup` registers a TF table with
    two frequencies per value, which changes every score that reads it.
    """
    first = scalar(
        standardized,
        f"SELECT count(*) FROM {TF_LOOKUP} WHERE model_version = ? AND tf_snapshot_id = ?",
        MODEL_VERSION,
        frozen,
    )

    again = materialize_tf_lookup(standardized, cfg, MODEL_VERSION, frozen)

    assert again == first
    duplicates = scalar(
        standardized,
        f"SELECT count(*) - count(DISTINCT (model_version, tf_snapshot_id, column_name, value)) "
        f"FROM {TF_LOOKUP}",
    )
    assert duplicates == 0

    # A different snapshot id under the same model_version is a different key and must
    # coexist: `er correct` re-materializes without retraining (S4.3.3).
    other = new_tf_snapshot_id()
    materialize_tf_lookup(standardized, cfg, MODEL_VERSION, other)
    assert scalar(standardized, f"SELECT count(*) FROM {TF_LOOKUP}") == first * 2


def test_missing_tf_lookup_is_a_precondition_failure(
    initialised_lake: duckdb.DuckDBPyConnection, cfg: Config
) -> None:
    """AC5: an unfrozen key stops the run at exit 3 rather than scoring on computed TF.

    On a lake with `tf_lookup` created and empty, which is the state a run reaches when
    it points at a `model_registry` row whose snapshot was never materialized — or was
    materialized and then reclaimed.
    """
    assert scalar(initialised_lake, f"SELECT count(*) FROM {TF_LOOKUP}") == 0

    with pytest.raises(MissingTfLookupError) as refusal:
        register_tf(object(), initialised_lake, cfg, MODEL_VERSION, new_tf_snapshot_id())

    assert tf_columns(cfg)[0] in str(refusal.value), "the refusal must name the missing column"
    # The CLI translates the class, not the exception type (S4.7), so the exit status
    # is asserted through the same function `er` uses.
    assert exit_code_for(refusal.value) == int(ExitCode.PRECONDITION) == 3


def test_registered_tf_scores_without_leaking_into_the_lake(
    frozen: str, standardized: duckdb.DuckDBPyConnection, cfg: Config
) -> None:
    """AC8: registering frozen TF and scoring leaves the lake, and `tf_lookup`, untouched.

    This is also the only check that the registered frame is shaped the way Splink
    reads it: a wrongly named frequency column would leave the TF adjustment silently
    unapplied, and the join it performs is the only thing that notices.
    """
    before = scalar(
        standardized,
        f"SELECT count(*) FROM {TF_LOOKUP} WHERE model_version = ? AND tf_snapshot_id = ?",
        MODEL_VERSION,
        frozen,
    )
    standardized.execute(
        f"CREATE OR REPLACE TABLE {FRAME} AS SELECT {', '.join(FRAME_COLUMNS)} FROM {STD_RECORDS}"
    )
    # `splink_api` issues `SET schema`, and this suite runs on the SESSION connection
    # every other suite shares. Restored rather than left pointing at the scratch
    # schema, so a later module cannot fail for a reason this one caused.
    search_path = scalar(standardized, "SELECT current_schema()")
    try:
        linker = Linker(FRAME, settings=build_settings(cfg), db_api=splink_api(standardized))
        registered = register_tf(linker, standardized, cfg, MODEL_VERSION, frozen)
        predictions = linker.inference.predict(
            threshold_match_probability=cfg.thresholds.review_low
        )
        scored = standardized.execute(
            f"SELECT count(*) FROM {predictions.physical_name}"
        ).fetchone()
    finally:
        standardized.execute(f"SET schema = '{search_path}'")

    assert registered == TF
    assert scored is not None and scored[0] > 0, "the prediction scored nothing; AC8 is vacuous"
    assert_no_splink_relations_in_lake(standardized)
    after = scalar(
        standardized,
        f"SELECT count(*) FROM {TF_LOOKUP} WHERE model_version = ? AND tf_snapshot_id = ?",
        MODEL_VERSION,
        frozen,
    )
    assert after == before
