"""`base_10`'s standardized corpus against its committed `std_hashes.csv` (S8.2.1, S8.3).

This is the first end-to-end use of a committed expectation. Everything before it
asserted a property the test could compute for itself; this one asserts that a real
`er ingest` + `dbt build` over `base_10` reproduces 23 digests that were captured
from an earlier run and committed, which is what makes T-STD-1's committed-file arm
a regression test rather than a tautology.

**How `expected/base/std_hashes.csv` is produced.** Never by hand. Every run of this
module writes the digest set it computed to
`artifacts/base_10/expected/base/std_hashes.csv` -- in S8.2.1's encoding, sorted the
way a committed file is stored -- *before* it compares, so the emitted file is
available on the host whether the comparison passed or failed (`artifacts/` is the
Compose bind mount of S7.1, and it is gitignored). Regenerating the expectation is
therefore two commands, and the second one is the deliberate act S8.2.1 asks a
fixture author to make:

```bash
bash scripts/ci/itest.sh tests/integration/scenarios/test_base_10_std.py -q
cp artifacts/base_10/expected/base/std_hashes.csv \
   fixtures/static/base_10/expected/base/std_hashes.csv
```

The emit is unconditional and the assertion is unconditional, so this affordance can
never turn a red run green: a digest that moved still fails here, and the diff it
produces is the point.

Two choices about how the corpus is built, both inherited from
`tests/integration/scenarios/test_int_std_records.py` and made for its reasons.
dbt is driven through `er.dbt_runner.run_dbt` with the connection detached for the
length of every invocation (S4.0b); `er standardize` is not the entry point because
it is still ER-014's `NoOpStage` and would build nothing. And the small `Dbt`
harness is duplicated from that module rather than imported: a test module importing
another test module makes a node id's dependencies invisible to whoever reads it.

The digest itself is `table_content_hash` (ER-044) applied row by row to the
`STD_HASH_COLUMNS` projection of `int_std_records` -- the same function T-STD-1's
determinism arm uses, so the two arms cannot disagree about what a `std_hash` is.
"""

from __future__ import annotations

import csv
import os
import subprocess
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import duckdb
import pytest
from helpers.expected import NULL_TOKEN, load_expected, sort_key
from helpers.scenario import EXPECTED_HEADERS, Scenario, expected_header_key, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, connect, detach
from er.lake.hashing import STD_HASH_COLUMNS, table_content_hash
from er.lake.model import SCHEMA_QUALIFIER

REPO_ROOT = Path(__file__).resolve().parents[3]

#: The S8.2 fixture this suite standardizes, its only phase, and the relation of
#: that phase this ticket commits.
SCENARIO_NAME = "base_10"
PHASE = "base"
RELATION = "std_hashes"

#: The relation the digests are computed over, and the two dbt selectors that build
#: it -- the selection `er standardize` will run once it is wired (S4.2).
MODEL = "int_std_records"
STAGING_SELECTOR = "staging"
INTERMEDIATE_SELECTOR = "intermediate"

#: S8.2's ground truth: 23 records over 10 personas, every one of them current.
TOTAL_ROWS = 23

#: Where a run leaves the digest set it computed. Under `artifacts/`, which is the
#: bind mount that reaches the host from inside the container (S7.1) and the one
#: directory a test may write to without dirtying the tree.
EMIT_PATH = REPO_ROOT / "artifacts" / SCENARIO_NAME / "expected" / PHASE / f"{RELATION}.csv"

#: S8.2.1's literal header for this file, read from ER-028's transcription of the
#: spec rather than retyped -- one authority for the header, not two.
HEADER: tuple[str, ...] = EXPECTED_HEADERS[expected_header_key(RELATION)]


def config() -> Config:
    """The validated S6 document this session runs against."""
    return load_config(Path(os.environ["ER_CONFIG"]))


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
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


def computed_hashes(connection: duckdb.DuckDBPyConnection) -> dict[tuple[str, str], str]:
    """`(source_system, source_record_id) -> std_hash` over the current corpus.

    The projection is `STD_HASH_COLUMNS` in its literal T-STD-1 order plus the two
    key columns, and the digest is computed in Python by `table_content_hash` rather
    than in SQL: S8.3 pins one preimage, and a second implementation of it in a
    `SELECT` would eventually disagree with the one the pipeline uses.
    """
    projection = ("source_system", "source_record_id", *STD_HASH_COLUMNS)
    rows = connection.execute(
        f"SELECT {', '.join(projection)} FROM {SCHEMA_QUALIFIER}.{MODEL}"
    ).fetchall()

    computed: dict[tuple[str, str], str] = {}
    for row in rows:
        values: Mapping[str, Any] = dict(zip(projection, row, strict=True))
        key = (str(values["source_system"]), str(values["source_record_id"]))
        assert key not in computed, f"{key}: two current rows for one record (S4.2)"
        computed[key] = table_content_hash(values)
    return computed


def emit(computed: Mapping[tuple[str, str], str], destination: Path) -> Path:
    """Write `computed` as an S8.2.1 expected file: literal header, stored sorted.

    Stored in the byte-wise ascending order of the full column tuple, so the emitted
    file is one a fixture author can copy over the committed one unedited -- a file
    that had to be re-sorted by hand before it could be committed would be a file
    that gets committed mis-sorted.
    """
    rows = [
        {"source_system": source_system, "source_record_id": source_record_id, "std_hash": digest}
        for (source_system, source_record_id), digest in computed.items()
    ]
    rows.sort(key=lambda row: sort_key(row, HEADER))

    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.writer(handle, lineterminator="\n")
        writer.writerow(HEADER)
        for row in rows:
            writer.writerow([row[column] or NULL_TOKEN for column in HEADER])
    return destination


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

    The image ships no `dbt/dbt_packages` (`.dockerignore`) and the intermediate
    build runs `dbt_utils` tests. A plain subprocess: `deps` touches no warehouse,
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
def standardized(
    base_10: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    tmp_path: Path,
) -> None:
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


def test_std_hashes_match_committed_expectation(base_10: Scenario, standardized: None) -> None:
    """AC2: the computed digest set equals the committed one, in both directions."""
    with connect() as connection:
        computed = computed_hashes(connection)

    # Emitted BEFORE the comparison, so a failing run still leaves the set it
    # computed on the host for a fixture author to inspect and, if the change was
    # intended, to copy over the committed file.
    emit(computed, EMIT_PATH)

    committed_path = base_10.expected_path(PHASE, RELATION)
    assert committed_path is not None, (
        f"expected/{PHASE}/{RELATION}.csv is not committed; this phase makes no claim to assert"
    )
    committed = {
        (str(row["source_system"]), str(row["source_record_id"])): row["std_hash"]
        for row in load_expected(committed_path)
    }

    assert len(computed) == TOTAL_ROWS, f"{len(computed)} current rows, expected {TOTAL_ROWS}"
    assert len(committed) == TOTAL_ROWS

    # Set equality in both directions, with the symmetric difference printed: the
    # failure a fixture author has to act on is WHICH triples moved, and a bare
    # `assert computed == committed` over 23 SHA-256s prints two walls of hex.
    computed_set = {(*key, digest) for key, digest in computed.items()}
    committed_set = {(*key, digest) for key, digest in committed.items()}
    only_computed = sorted(computed_set - committed_set)
    only_committed = sorted(committed_set - computed_set)
    assert not (only_computed or only_committed), (
        f"std_hash sets differ.\n"
        f"  computed but not committed ({len(only_computed)}): {only_computed}\n"
        f"  committed but not computed ({len(only_committed)}): {only_committed}\n"
        f"The set this run computed was written to {EMIT_PATH.relative_to(REPO_ROOT)}; "
        f"copy it over {committed_path.relative_to(REPO_ROOT)} only if the change was intended."
    )
