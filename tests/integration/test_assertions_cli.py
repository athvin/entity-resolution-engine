"""`er assert` against a real lake (S4.0, S4.4, S5.0, S8.2.1).

This suite is about the *lifecycle of a steward's correction*, which is the thing
S4.4 says makes a correction survive a re-run. What is asserted here is everything
that has to be true of the rows the command leaves behind:

* the pair is canonicalised on write, so `rec_a_key < rec_b_key` holds however the
  operator typed `--a` and `--b` (S5.0, D9);
* precedence is enforced at WRITE time: `never` and `always` cannot both be active
  for one pair, and the second insert is rejected (exit `1`) naming the row that
  already holds the pair — not silently ordered around (S4.4);
* a malformed key or an unknown kind exits `2` and writes nothing (S4.0);
* `remove` is a STAMP, never a `DELETE`: `active=false` with `retracted_by`/
  `retracted_at`, the row count unchanged, and the pair free for the opposite kind;
* `load` applies every row of an S8.2.1 file and is idempotent — a second load of
  the same file exits `10` and writes zero rows;
* and the S5.0 logical keys `dbt test --select tag:keys` enforces stay green over
  everything the command wrote, because "at most one active row per pair" is a dbt
  test with no constraint behind it (B2) and is therefore this writer's to keep.

Every invocation below is the installed `er` console script in this session's
namespace, reaching a real DuckLake through Postgres. The module under test is
`er.review.assertions`; nothing here reimplements a query it owns.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest

from er.errors import ExitCode
from er.lake.dbt_sources import KEYS_TAG
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.review.assertions import ALWAYS, ASSERTIONS_CSV_HEADER, NEVER, active_assertions

REPO_ROOT: Final = Path(__file__).resolve().parents[2]
DBT_DIR: Final = REPO_ROOT / "dbt"
DBT_PROFILES_DIR: Final = DBT_DIR / "profiles"

ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.assertions"

#: The pair AC1 names, deliberately given to the CLI the wrong way round:
#: `webforms:9` sorts after `crm:1`, so "canonicalised, not reflected verbatim" is
#: only observable against a caller that got the order wrong.
KEY_A: Final = "webforms:9"
KEY_B: Final = "crm:1"
CANONICAL: Final = (KEY_B, KEY_A)

#: A third record, for the pair that stays untouched while another is rejected.
KEY_C: Final = "billing:7"

#: The `keys` selector T-KEY-1a gates on, and the exclusion that keeps it to the
#: `ddl.py`-owned arm: this module builds no dbt model, and a test attached to a
#: relation that was never built ERRORs — reporting a missing build as a failed key.
KEYS_SELECTOR: Final = f"tag:{KEYS_TAG}"
KEYS_EXCLUSION: Final = "resource_type:model"

_TOTAL_RE: Final = re.compile(r"TOTAL=(\d+)")


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def stdout_records(result: subprocess.CompletedProcess[str]) -> list[dict[str, Any]]:
    """The `--json` objects on stdout, in emission order."""
    return [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def row_count(connection: duckdb.DuckDBPyConnection) -> int:
    """Every `assertions` row, retracted ones included — this relation never shrinks."""
    return int(scalar(connection, f"SELECT count(*) FROM {ASSERTIONS}"))


def write_assertions_csv(path: Path, *lines: str) -> Path:
    """An S8.2.1 `assertions.csv` under its literal header."""
    path.write_text("\n".join([ASSERTIONS_CSV_HEADER, *lines]) + "\n", encoding="utf-8")
    return path


@contextmanager
def _detached(connection: duckdb.DuckDBPyConnection) -> Iterator[None]:
    """Hold the lake detached from ``connection`` for the body (S4.0b).

    No Python DuckDB connection may span a dbt invocation. Re-attaching in a
    ``finally`` is what stops a failed invocation from breaking the *next* test and
    S8.1's namespace reclamation rather than this one.
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


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored.

    Conditional so a developer who has run `dbt deps` once does not need the network
    on every run: nine of the S5.0 keys are `dbt_utils.unique_combination_of_columns`,
    and the image deliberately ships no `dbt/dbt_packages`.
    """
    if (DBT_DIR / "dbt_packages" / "dbt_utils").is_dir():
        return
    completed = _dbt("deps")
    assert completed.returncode == 0, completed.stdout


def test_assert_add_remove_lifecycle(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC1/AC4: one canonical row on add; a stamped retraction that frees the pair."""
    added = run_er(
        "assert", "add", "--a", KEY_A, "--b", KEY_B, "--kind", ALWAYS, "--by", "tester", "--json"
    )
    assert added.returncode == int(ExitCode.SUCCESS), added.stdout + added.stderr

    (record,) = stdout_records(added)
    # S4.0's stdout column for `er assert`, exactly: five fields and no sixth.
    assert set(record) == {"assertion_id", "rec_a_key", "rec_b_key", "kind", "active"}
    assert (record["rec_a_key"], record["rec_b_key"]) == CANONICAL
    assert record["kind"] == ALWAYS
    assert record["active"] is True

    assert row_count(initialised_lake) == 1
    stored = scalar(
        initialised_lake,
        f"SELECT created_by FROM {ASSERTIONS} WHERE assertion_id = ?",
        record["assertion_id"],
    )
    assert stored == "tester"
    live = active_assertions(initialised_lake)
    assert [row.pair for row in live] == [CANONICAL]
    assert live[0].rec_a_key < live[0].rec_b_key

    removed = run_er(
        "assert", "remove", "--assertion-id", record["assertion_id"], "--by", "tester", "--json"
    )
    assert removed.returncode == int(ExitCode.SUCCESS), removed.stdout + removed.stderr
    assert stdout_records(removed)[0]["active"] is False

    # Never deleted: the row count is unchanged and the stamps are populated, which is
    # what makes the assertion delta between two runs computable (S4.4, S4.5.1).
    assert row_count(initialised_lake) == 1
    retracted_by, retracted_at = initialised_lake.execute(
        f"SELECT retracted_by, retracted_at FROM {ASSERTIONS} WHERE assertion_id = ?",
        [record["assertion_id"]],
    ).fetchall()[0]
    assert retracted_by == "tester"
    assert retracted_at is not None
    assert active_assertions(initialised_lake) == []

    # The pair is now free for the opposite kind, which it was not before the retraction.
    opposite = run_er(
        "assert", "add", "--a", KEY_A, "--b", KEY_B, "--kind", NEVER, "--by", "tester", "--json"
    )
    assert opposite.returncode == int(ExitCode.SUCCESS), opposite.stdout + opposite.stderr
    assert row_count(initialised_lake) == 2
    assert [(row.pair, row.kind) for row in active_assertions(initialised_lake)] == [
        (CANONICAL, NEVER)
    ]


def test_assert_add_exit_codes(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC2/AC3: a conflicting insert exits 1; a malformed key or kind exits 2."""
    first = run_er(
        "assert", "add", "--a", KEY_A, "--b", KEY_B, "--kind", ALWAYS, "--by", "tester", "--json"
    )
    assert first.returncode == int(ExitCode.SUCCESS), first.stdout + first.stderr
    existing_id = stdout_records(first)[0]["assertion_id"]

    # The pair given the other way round as well, so the rejection is about the PAIR
    # and not about the argument order.
    conflict = run_er(
        "assert", "add", "--a", KEY_B, "--b", KEY_A, "--kind", NEVER, "--by", "tester"
    )
    assert conflict.returncode == int(ExitCode.STAGE_FAILURE), conflict.stdout + conflict.stderr
    # An operator's next action is `er assert remove --assertion-id <this one>`, so the
    # refusal has to name it rather than leave them querying the lake for it.
    assert existing_id in conflict.stderr

    assert row_count(initialised_lake) == 1
    assert [(row.assertion_id, row.kind) for row in active_assertions(initialised_lake)] == [
        (existing_id, ALWAYS)
    ]

    # A key with a second ':' -- `split_record_key` splits on the first separator only,
    # so this is the case that would otherwise pass as ('crm', '1:2') (S5.0, D6).
    malformed = run_er(
        "assert", "add", "--a", "crm:1:2", "--b", KEY_C, "--kind", ALWAYS, "--by", "tester"
    )
    assert malformed.returncode == int(ExitCode.CONFIG), malformed.stdout + malformed.stderr

    unknown_kind = run_er(
        "assert", "add", "--a", KEY_C, "--b", KEY_B, "--kind", "maybe", "--by", "tester"
    )
    assert unknown_kind.returncode == int(ExitCode.CONFIG), (
        unknown_kind.stdout + unknown_kind.stderr
    )

    # Neither refusal wrote anything: the only row is still the one that succeeded.
    assert row_count(initialised_lake) == 1


def test_assert_load_is_idempotent(
    initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    """AC5: `load` applies every row of an S8.2.1 file; a second load exits 10."""
    path = write_assertions_csv(
        tmp_path / "assertions.csv",
        f"base,{KEY_A},{KEY_B},{ALWAYS},tester,\\N",
        f"batch,{KEY_C},{KEY_B},{NEVER},tester,keep apart",
    )

    loaded = run_er("assert", "load", "--path", str(path), "--json")
    assert loaded.returncode == int(ExitCode.SUCCESS), loaded.stdout + loaded.stderr

    # One line per row, and every row applied -- `load` does not filter by phase: the
    # `phase` column exists so a scenario driver can slice the file first (S8.2.1).
    records = stdout_records(loaded)
    assert len(records) == 2
    assert {(record["rec_a_key"], record["rec_b_key"]) for record in records} == {
        CANONICAL,
        (KEY_C, KEY_B),
    }
    assert row_count(initialised_lake) == 2
    for row in active_assertions(initialised_lake):
        assert row.rec_a_key < row.rec_b_key
    notes = dict(initialised_lake.execute(f"SELECT rec_a_key, note FROM {ASSERTIONS}").fetchall())
    # `\N` is NULL; a real note is itself (S8.2.1's encoding rules).
    assert notes[CANONICAL[0]] is None
    assert notes[KEY_C] == "keep apart"

    again = run_er("assert", "load", "--path", str(path), "--json")

    assert again.returncode == int(ExitCode.NOTHING_TO_DO), again.stdout + again.stderr
    assert stdout_records(again) == []
    assert row_count(initialised_lake) == 2


def test_keys_tests_stay_green_over_the_rows_er_assert_wrote(
    initialised_lake: duckdb.DuckDBPyConnection, dbt_packages: None
) -> None:
    """AC8: `dbt test --select tag:keys` is green after the command has written.

    The two claims it makes about `assertions` are the two this writer keeps with no
    constraint behind it (B2, S5.0): at most one `active` row per `(rec_a_key,
    rec_b_key)`, and `rec_a_key < rec_b_key` on every row. Both are only observable
    once the relation holds a retracted row beside an active one for the SAME pair —
    a shape a naive filtered-uniqueness implementation gets wrong.
    """
    added = run_er(
        "assert", "add", "--a", KEY_A, "--b", KEY_B, "--kind", ALWAYS, "--by", "tester", "--json"
    )
    assert added.returncode == int(ExitCode.SUCCESS), added.stdout + added.stderr
    assertion_id = stdout_records(added)[0]["assertion_id"]

    removed = run_er("assert", "remove", "--assertion-id", assertion_id, "--by", "tester")
    assert removed.returncode == int(ExitCode.SUCCESS), removed.stdout + removed.stderr

    # Two rows for one pair, exactly one of them active -- plus a second pair, so the
    # uniqueness test has more than one group to be right about.
    reasserted = run_er(
        "assert", "add", "--a", KEY_B, "--b", KEY_A, "--kind", NEVER, "--by", "tester"
    )
    assert reasserted.returncode == int(ExitCode.SUCCESS), reasserted.stdout + reasserted.stderr
    other = run_er("assert", "add", "--a", KEY_C, "--b", KEY_B, "--kind", ALWAYS, "--by", "tester")
    assert other.returncode == int(ExitCode.SUCCESS), other.stdout + other.stderr
    assert row_count(initialised_lake) == 3

    with _detached(initialised_lake):
        completed = _dbt(
            "test", "--select", KEYS_SELECTOR, "--exclude", KEYS_EXCLUSION, "--target", "lake"
        )

    assert completed.returncode == 0, completed.stdout
    total = _TOTAL_RE.search(completed.stdout)
    assert total is not None, f"no summary line in dbt's output:\n{completed.stdout}"
    # A selector that matches nothing exits 0 and proves nothing (S12 M1).
    assert int(total.group(1)) > 0
