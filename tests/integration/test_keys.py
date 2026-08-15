"""T-KEY-1a: a `ddl.py`-owned logical key is enforced by a test, not by the engine (S8.3).

Two claims, and the second is why the first has to exist. `raw_records`' key
``(source_system, source_record_id, content_hash)`` is declared in S5.0 and appears
nowhere in the DDL, so a duplicate lands silently and only
``dbt test --select tag:keys`` reports it — and DuckLake refuses `PRIMARY KEY`
outright, which is B2 in executable form.

T-KEY-1b, the dbt-owned arm, is deliberately not here: it needs `int_std_records`,
which does not exist until M2, and a milestone may only gate on relations it creates
(S12).

The small helpers below are local rather than shared with
`tests/integration/test_logical_keys.py`. `tests/integration/conftest.py` belongs to
another ticket, and the alternative — one test module importing another — makes a
node id's dependencies invisible to whoever reads it.
"""

from __future__ import annotations

import subprocess
from collections.abc import Iterator
from contextlib import contextmanager
from datetime import datetime
from pathlib import Path

import duckdb
import pytest

from er.lake.dbt_sources import KEYS_TAG
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER

REPO_ROOT = Path(__file__).resolve().parents[2]
DBT_DIR = REPO_ROOT / "dbt"
DBT_PROFILES_DIR = DBT_DIR / "profiles"

KEYS_SELECTOR = f"tag:{KEYS_TAG}"

#: The relation B2 is about: DuckLake supports no `PRIMARY KEY`, so this statement
#: MUST raise. Named `t_pk` per the acceptance criterion.
PK_RELATION = "t_pk"

# The one duplicated key, written twice. Every other column is identical too — the
# key is what the test is about, and varying anything else would leave a reader
# wondering which difference dbt objected to.
_DUPLICATE = {
    "source_system": "crm",
    "source_record_id": "1",
    "payload": "{}",
    "content_hash": "0" * 64,
    "is_deleted": False,
    "ingest_batch_id": "batch_1",
    "ingested_at": datetime(2026, 1, 1),
}


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored.

    The image ships no `dbt/dbt_packages` (docker/.dockerignore), and the key this
    test violates is enforced by `dbt_utils.unique_combination_of_columns`.
    """
    if (DBT_DIR / "dbt_packages" / "dbt_utils").is_dir():
        return
    completed = _dbt("deps")
    assert completed.returncode == 0, completed.stdout


@contextmanager
def _detached(connection: duckdb.DuckDBPyConnection) -> Iterator[None]:
    """Hold the lake detached for the body: no Python connection spans dbt (S4.0b)."""
    detach(connection)
    try:
        yield
    finally:
        for statement in attach_statements():
            connection.execute(statement)


def _dbt(command: str, *arguments: str) -> subprocess.CompletedProcess[str]:
    argv = [
        "dbt",
        command,
        "--project-dir",
        str(DBT_DIR),
        "--profiles-dir",
        str(DBT_PROFILES_DIR),
        *arguments,
    ]
    return subprocess.run(argv, capture_output=True, text=True, check=False, cwd=REPO_ROOT)


def _insert_raw_record(connection: duckdb.DuckDBPyConnection) -> None:
    columns = ", ".join(_DUPLICATE)
    placeholders = ", ".join("?" for _ in _DUPLICATE)
    connection.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.raw_records ({columns}) VALUES ({placeholders})",
        list(_DUPLICATE.values()),
    )


def _relation_exists(connection: duckdb.DuckDBPyConnection, name: str) -> bool:
    database, schema = SCHEMA_QUALIFIER.split(".", 1)
    row = connection.execute(
        "SELECT count(*) FROM duckdb_tables() "
        "WHERE database_name = ? AND schema_name = ? AND table_name = ?",
        [database, schema, name],
    ).fetchone()
    return bool(row is not None and row[0])


def test_ddl_owned_duplicate_key_fails_dbt_test(
    dbt_packages: None, initialised_lake: duckdb.DuckDBPyConnection
) -> None:
    """T-KEY-1a: the duplicate lands, the engine says nothing, and `tag:keys` goes red."""
    _insert_raw_record(initialised_lake)
    with _detached(initialised_lake):
        clean = _dbt("test", "--select", KEYS_SELECTOR, "--target", "lake")
    assert clean.returncode == 0, clean.stdout

    # The engine accepts the duplicate: `raw_records`' key is logical, and this
    # INSERT is the whole reason it has to be a test (S5.0).
    _insert_raw_record(initialised_lake)
    with _detached(initialised_lake):
        duplicated = _dbt("test", "--select", KEYS_SELECTOR, "--target", "lake")
    assert duplicated.returncode != 0, duplicated.stdout
    assert "raw_records" in duplicated.stdout

    # B2, executable: DuckLake supports no PRIMARY KEY. Asserted as a raise and as
    # an absent relation, and then bracketed by the same statement without the
    # constraint — which succeeds, so the refusal is the constraint and not the
    # shape of the statement or the state of the namespace.
    with pytest.raises(duckdb.Error):
        initialised_lake.execute(
            f"CREATE TABLE {SCHEMA_QUALIFIER}.{PK_RELATION} (a VARCHAR PRIMARY KEY)"
        )
    assert not _relation_exists(initialised_lake, PK_RELATION)

    initialised_lake.execute(f"CREATE TABLE {SCHEMA_QUALIFIER}.{PK_RELATION} (a VARCHAR NOT NULL)")
    assert _relation_exists(initialised_lake, PK_RELATION)
