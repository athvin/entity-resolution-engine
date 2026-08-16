"""The S5.0 logical keys, executed against a real DuckLake namespace (S5.0, S8.1, S12).

B2 is that DuckLake enforces `NOT NULL` and nothing else, so every key of S5.0 is a
dbt test or it is nothing. A rendered `sources.yml` proves only that the tests were
*declared*; this module is where each one is shown to fire. Every test here inserts
a row that violates exactly one claim and asserts that
``dbt test --select tag:keys`` — T-KEY-1a's own gate — goes red, because a selector
that matches nothing exits 0 and proves nothing (S12 M1).

dbt runs as a plain subprocess and the lake is **detached from the Python connection
first** (S4.0b): no Python DuckDB connection may span a dbt invocation.
:func:`_detached` re-attaches on every exit path, so the session handle S8.1 hands
out survives a failing invocation and the namespace is still reclaimed.

`dbt deps` is a fixture rather than a precondition because the image deliberately
ships no `dbt/dbt_packages` (docker/.dockerignore: "`dbt deps` regenerates them
in-container"), and nine of the S5.0 keys are `dbt_utils.unique_combination_of_columns`.
"""

from __future__ import annotations

import re
import subprocess
from collections.abc import Iterator, Mapping
from contextlib import contextmanager
from datetime import date, datetime
from pathlib import Path
from typing import Any

import duckdb
import pytest
import yaml

from er.lake.dbt_sources import KEYS_TAG, render_sources_yml
from er.lake.ducklake import attach_statements, detach
from er.lake.model import PAIR_RELATIONS, REGISTRY, SCHEMA_QUALIFIER, TableSpec

REPO_ROOT = Path(__file__).resolve().parents[2]
DBT_DIR = REPO_ROOT / "dbt"
DBT_PROFILES_DIR = DBT_DIR / "profiles"
SINGULAR_TESTS = DBT_DIR / "tests"

# Absolute rather than `er.dbt_runner`'s relative `"dbt"`: that constant is resolved
# against the process working directory, and a test must not depend on pytest having
# been invoked from the repository root.
KEYS_SELECTOR = f"tag:{KEYS_TAG}"
PAIR_ORDERING_TEST = "assert_canonical_pair_ordering"

# T-KEY-1a is the `ddl.py`-owned arm, and this module builds no dbt model: function
# isolation drops the dbt-owned relations (S8.1) and nothing here recreates them. From
# M2 the `keys` tag also covers model-attached tests -- `stg_<source>`'s logical key is
# a `dbt_utils.unique_combination_of_columns` like every other -- and those ERROR
# against a relation that was never built, which would report a missing *build* as a
# failed *key*. Excluding the model nodes removes their attached tests with them, so
# what runs here is exactly the renderer's source tests plus the singular ones, which
# is what `_rendered_keys_tests` counts. The dbt-owned arm is asserted where those
# relations exist, in `tests/integration/scenarios/test_staging.py` (S12's arm rule).
KEYS_EXCLUSION = "resource_type:model"

# dbt's own summary line, which is the only place the number of tests it actually
# ran is reported: `Done. PASS=34 WARN=0 ERROR=0 SKIP=0 TOTAL=34`.
_TOTAL_RE = re.compile(r"TOTAL=(\d+)")

# A pair written the wrong way round, and the same pair canonicalised (S5.0, D9).
UNORDERED_PAIR = ("b:2", "a:1")
ORDERED_PAIR = ("a:1", "b:2")

# One value per S5 type, used to fill the NOT NULL columns a test does not care
# about. `VARCHAR` is derived from the column name so that a relation's two pair
# columns come out already canonical (`x_rec_a_key` < `x_rec_b_key`) and a test about
# some other key is not also a pair-ordering violation.
_FILLERS: Mapping[str, Any] = {
    "BIGINT": 1,
    "DOUBLE": 0.5,
    "BOOLEAN": True,
    "DATE": date(2026, 1, 1),
    "TIMESTAMP": datetime(2026, 1, 1),
    "JSON": "{}",
    "LIST(VARCHAR)": ["x"],
}

#: AC7's three domains, one per shape: a status, a stage name and a disposition.
DOMAIN_VIOLATIONS = (
    ("entities", "status", "bogus"),
    ("run_stages", "stage", "doctor"),
    ("er_touched_entities", "disposition", "keep"),
)


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored.

    Conditional so a developer who has run `dbt deps` once does not need the network
    on every run, and so CI's static job — which runs it on the runner — does not
    make the container's copy stale.
    """
    if (DBT_DIR / "dbt_packages" / "dbt_utils").is_dir():
        return
    completed = _dbt("deps")
    assert completed.returncode == 0, completed.stdout


@contextmanager
def _detached(connection: duckdb.DuckDBPyConnection) -> Iterator[None]:
    """Hold the lake detached from ``connection`` for the body (S4.0b).

    Re-attaching in a ``finally`` matters: a dbt invocation that fails is the normal
    case in this module, and a connection left detached would fail the *next* test
    and S8.1's namespace reclamation rather than this one.
    """
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


def _dbt_test(
    connection: duckdb.DuckDBPyConnection, select: str = KEYS_SELECTOR
) -> subprocess.CompletedProcess[str]:
    with _detached(connection):
        return _dbt("test", "--select", select, "--exclude", KEYS_EXCLUSION, "--target", "lake")


def _total(completed: subprocess.CompletedProcess[str]) -> int:
    """How many tests dbt reported running."""
    match = _TOTAL_RE.search(completed.stdout)
    assert match is not None, f"no summary line in dbt's output:\n{completed.stdout}"
    return int(match.group(1))


def _rendered_keys_tests() -> int:
    """How many `keys`-tagged tests the renderer emitted, plus the two singular ones.

    Counted from :func:`~er.lake.dbt_sources.render_sources_yml` rather than from the
    committed file, so this is the renderer's own claim; the unit parity test is what
    binds the committed file to it.
    """
    document = yaml.safe_load(render_sources_yml(REGISTRY.values()))
    tagged = 0
    for table in document["sources"][0]["tables"]:
        columns = [column.get("data_tests") for column in table["columns"]]
        for entries in [table.get("data_tests"), *columns]:
            for entry in entries or ():
                if not isinstance(entry, dict):
                    continue
                (body,) = entry.values()
                if KEYS_TAG in ((body or {}).get("config") or {}).get("tags", []):
                    tagged += 1
    singular = [
        path
        for path in sorted(SINGULAR_TESTS.glob("*.sql"))
        if f"tags=['{KEYS_TAG}']" in path.read_text(encoding="utf-8")
    ]
    return tagged + len(singular)


def _row(spec: TableSpec, **overrides: Any) -> dict[str, Any]:
    """A row satisfying every NOT NULL column and every domain of ``spec``.

    Built from the registry so that the only thing wrong with a row is the override
    the caller named: a hand-written column list would drift from S5 and start
    failing a second test for a second reason.
    """
    row: dict[str, Any] = {}
    for column in spec.columns:
        if column.name in overrides:
            row[column.name] = overrides[column.name]
            continue
        if column.nullable:
            continue
        domain = spec.enums.get(column.name)
        if domain is not None:
            row[column.name] = sorted(domain)[0]
        elif column.type == "VARCHAR":
            row[column.name] = f"x_{column.name}"
        else:
            row[column.name] = _FILLERS[column.type]
    return row


def _insert(connection: duckdb.DuckDBPyConnection, relation: str, **overrides: Any) -> None:
    row = _row(REGISTRY[relation], **overrides)
    columns = ", ".join(row)
    placeholders = ", ".join("?" for _ in row)
    connection.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.{relation} ({columns}) VALUES ({placeholders})",
        list(row.values()),
    )


def _truncate(connection: duckdb.DuckDBPyConnection, relation: str) -> None:
    connection.execute(f"DELETE FROM {SCHEMA_QUALIFIER}.{relation}")


def _failing_pair_relations(connection: duckdb.DuckDBPyConnection) -> set[str]:
    """The relations dbt's own compiled pair-ordering query reports as unordered.

    dbt prints that a singular test failed but not which rows failed it, and AC6 is
    about the row set naming the relation. Reading the query out of
    ``target/compiled`` and running it is dbt's own artifact, not a second
    implementation of the test.
    """
    compiled = sorted((DBT_DIR / "target" / "compiled").rglob(f"{PAIR_ORDERING_TEST}.sql"))
    assert compiled, "dbt wrote no compiled SQL for the pair-ordering test"
    rows = connection.execute(compiled[0].read_text(encoding="utf-8")).fetchall()
    return {str(row[0]) for row in rows}


def test_keys_selector_is_non_empty_and_green(
    dbt_packages: None, initialised_lake: duckdb.DuckDBPyConnection
) -> None:
    """AC3: on a clean namespaced lake the selector matches every key test and passes."""
    completed = _dbt_test(initialised_lake)
    assert completed.returncode == 0, completed.stdout

    ran = _total(completed)
    assert ran > 0, "a selector that matches nothing exits 0 and proves nothing (S12 M1)"
    assert ran == _rendered_keys_tests()


@pytest.mark.parametrize("relation", sorted(PAIR_RELATIONS))
def test_pair_ordering_violation_fails_singular_test(
    dbt_packages: None, initialised_lake: duckdb.DuckDBPyConnection, relation: str
) -> None:
    """AC6: an uncanonicalised pair row fails, naming its relation; the canonical one passes."""
    # `review_queue` also holds entity subjects, whose pair keys are NULL; only its
    # `subject_type='pair'` rows carry the invariant (S5.0).
    subject: dict[str, Any] = {"subject_type": "pair"} if relation == "review_queue" else {}
    _insert(
        initialised_lake,
        relation,
        rec_a_key=UNORDERED_PAIR[0],
        rec_b_key=UNORDERED_PAIR[1],
        **subject,
    )

    failed = _dbt_test(initialised_lake)
    assert failed.returncode != 0, failed.stdout
    assert PAIR_ORDERING_TEST in failed.stdout
    assert _failing_pair_relations(initialised_lake) == {relation}

    _truncate(initialised_lake, relation)
    _insert(
        initialised_lake,
        relation,
        rec_a_key=ORDERED_PAIR[0],
        rec_b_key=ORDERED_PAIR[1],
        **subject,
    )
    passed = _dbt_test(initialised_lake, PAIR_ORDERING_TEST)
    assert passed.returncode == 0, passed.stdout


@pytest.mark.parametrize(("relation", "column", "value"), DOMAIN_VIOLATIONS)
def test_accepted_values_rejects_unknown_domain_value(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    relation: str,
    column: str,
    value: str,
) -> None:
    """AC7: every ``∈ {…}`` domain of the S5 DDL is enforced by a test in `tag:keys`."""
    assert value not in REGISTRY[relation].enums[column]
    _insert(initialised_lake, relation, **{column: value})

    completed = _dbt_test(initialised_lake)
    assert completed.returncode != 0, completed.stdout
    assert f"accepted_values_lake_{relation}_{column}" in completed.stdout


def test_filtered_uniqueness_on_active_assertions(
    dbt_packages: None, initialised_lake: duckdb.DuckDBPyConnection
) -> None:
    """M6: `assertions`' second key is unique only *where active*, and really is.

    Both halves matter. Without the filter the test would fail the first time a pair
    was asserted, retracted and asserted again, which is an ordinary steward action;
    without the test the second *active* row would make the assertion set for a pair
    depend on row order.
    """
    pair = {"rec_a_key": ORDERED_PAIR[0], "rec_b_key": ORDERED_PAIR[1]}
    _insert(initialised_lake, "assertions", assertion_id="A1", active=True, **pair)
    _insert(initialised_lake, "assertions", assertion_id="A2", active=False, **pair)

    retracted = _dbt_test(initialised_lake)
    assert retracted.returncode == 0, retracted.stdout

    _insert(initialised_lake, "assertions", assertion_id="A3", active=True, **pair)
    duplicated = _dbt_test(initialised_lake)
    assert duplicated.returncode != 0, duplicated.stdout
    assert "unique_combination_of_columns_lake_assertions" in duplicated.stdout
