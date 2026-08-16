"""`er review` against a real lake (S4.0, S4.3.5, S4.4, S5.0, S11).

This suite is about the mechanism S4.3.5 calls "resolving a row writes the
corresponding `assertions` row" — the thing that makes a steward's answer survive
the next run rather than being re-asked every time `er match` runs. What is
asserted here is everything that has to be true of the rows the command leaves
behind:

* `--as match` writes exactly one `active` `always` assertion for the row's
  canonical pair and `--as no_match` a `never`, while `--as dismiss` writes none —
  a dismissal is "not a steward task", not a claim about the two records;
* when the assertion is REJECTED by S4.4's precedence rule, the command exits `1`
  and the `review_queue` row is still `open` with a NULL `resolved_at`, with the
  `assertions` row count unchanged on both sides of the attempt. The failure is
  *injected* — an active `never` is asserted first — rather than inspected;
* `list` prints S4.0's five columns, caps at `--limit`, and exits `10` on an empty
  queue, while `resolve` against an unknown `review_id` exits `2`;
* an S11 entity subject has no pair to assert over, so `--as match` exits `2`
  against it and only `--as dismiss` is legal;
* and the S5.0 logical keys `dbt test --select tag:keys` enforces stay green with
  one canonical pair open for two reasons at once — the shape the filtered
  uniqueness key exists for (M20), and one a key without `reason` in it gets wrong.

Every invocation below is the installed `er` console script in this session's
namespace, reaching a real DuckLake through Postgres. The rows it resolves are
seeded through `er.review.queue`'s own upsert; nothing here reimplements a write
or a query either module owns.
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
from er.review.assertions import ALWAYS, NEVER, active_assertions
from er.review.queue import (
    COHERENCE,
    DISMISS,
    DISMISSED,
    ENTITY,
    GRAY_BAND,
    MATCH,
    NEVER_UNSATISFIABLE,
    NO_MATCH,
    OPEN,
    PAIR,
    RESOLVED_MATCH,
    RESOLVED_NO_MATCH,
    GrayBandPair,
    ReviewRow,
    upsert_entity_finding,
    upsert_escalation,
    upsert_gray_band_pairs,
)

REPO_ROOT: Final = Path(__file__).resolve().parents[2]
DBT_DIR: Final = REPO_ROOT / "dbt"
DBT_PROFILES_DIR: Final = DBT_DIR / "profiles"

ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.assertions"
REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.review_queue"

#: Three distinct pairs, one per resolution verb, so the three writes cannot
#: interfere: `always` and `never` for one pair are exactly what S4.4 rejects.
PAIR_MATCH: Final = ("webforms:9", "crm:1")
PAIR_NO_MATCH: Final = ("crm:2", "billing:7")
PAIR_DISMISS: Final = ("webforms:3", "billing:8")

#: The entity an S11 coherence finding stands for.
ENTITY_ID: Final = "01JQZ8XKQ4T7VN3M2B9CDEFGHE"

#: Two runs, so a refresh is a change between two known values.
RUN_1: Final = "01JQZ8XKQ4T7VN3M2B9CDEFGH1"

#: A `predict()` payload carrying what S4.3.5 requires to be retained.
WATERFALL: Final[dict[str, Any]] = {
    "gamma_email": 2,
    "gamma_family_name": 1,
    "bf_email": 41.7,
    "bf_family_name": 3.25,
}

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


def assertion_count(connection: duckdb.DuckDBPyConnection) -> int:
    """Every `assertions` row, retracted ones included — this relation never shrinks."""
    return int(scalar(connection, f"SELECT count(*) FROM {ASSERTIONS}"))


def seed_gray_band(
    connection: duckdb.DuckDBPyConnection, *pairs: tuple[str, str]
) -> list[ReviewRow]:
    """One open `gray_band` row per pair, written by the module that owns the write."""
    result = upsert_gray_band_pairs(
        connection,
        [
            GrayBandPair(rec_a_key=a, rec_b_key=b, match_probability=0.9, waterfall=WATERFALL)
            for a, b in pairs
        ],
        run_id=RUN_1,
    )
    assert result.added == len(pairs)
    return list(result.inserted)


def review_status(connection: duckdb.DuckDBPyConnection, review_id: str) -> tuple[Any, Any, Any]:
    """`(status, resolved_by, resolved_at)` for one row."""
    row = connection.execute(
        f"SELECT status, resolved_by, resolved_at FROM {REVIEW_QUEUE} WHERE review_id = ?",
        [review_id],
    ).fetchone()
    assert row is not None, f"no review_queue row for {review_id!r}"
    return (row[0], row[1], row[2])


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
    on every run: the S5.0 keys under test are `dbt_utils` tests, and the image
    deliberately ships no `dbt/dbt_packages`.
    """
    if (DBT_DIR / "dbt_packages" / "dbt_utils").is_dir():
        return
    completed = _dbt("deps")
    assert completed.returncode == 0, completed.stdout


def test_resolve_match_writes_assertion_in_one_transaction(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC4: `match` writes an `always`, `no_match` a `never`, `dismiss` neither."""
    matched, refused, dismissed = seed_gray_band(
        initialised_lake, PAIR_MATCH, PAIR_NO_MATCH, PAIR_DISMISS
    )

    resolved = run_er(
        "review",
        "resolve",
        "--review-id",
        matched.review_id,
        "--as",
        MATCH,
        "--by",
        "tester",
        "--json",
    )
    assert resolved.returncode == int(ExitCode.SUCCESS), resolved.stdout + resolved.stderr

    (record,) = stdout_records(resolved)
    # S4.0's stdout column for `er review`, exactly: five fields and no sixth.
    assert set(record) == {"review_id", "subject_type", "keys", "match_probability", "status"}
    assert record["status"] == RESOLVED_MATCH
    assert record["subject_type"] == PAIR

    status, resolved_by, resolved_at = review_status(initialised_lake, matched.review_id)
    assert (status, resolved_by) == (RESOLVED_MATCH, "tester")
    assert resolved_at is not None

    no_match = run_er(
        "review", "resolve", "--review-id", refused.review_id, "--as", NO_MATCH, "--by", "tester"
    )
    assert no_match.returncode == int(ExitCode.SUCCESS), no_match.stdout + no_match.stderr
    assert review_status(initialised_lake, refused.review_id)[0] == RESOLVED_NO_MATCH

    # Exactly one active assertion per resolved pair, of the kind the verb names, over
    # the CANONICAL pair (S5.0, D9) rather than over the keys as they were typed.
    live = active_assertions(initialised_lake)
    assert [(row.pair, row.kind) for row in live] == [
        (tuple(sorted(PAIR_NO_MATCH)), NEVER),
        (tuple(sorted(PAIR_MATCH)), ALWAYS),
    ]
    assert assertion_count(initialised_lake) == 2

    before = assertion_count(initialised_lake)
    dismiss = run_er(
        "review", "resolve", "--review-id", dismissed.review_id, "--as", DISMISS, "--by", "tester"
    )

    assert dismiss.returncode == int(ExitCode.SUCCESS), dismiss.stdout + dismiss.stderr
    assert review_status(initialised_lake, dismissed.review_id)[0] == DISMISSED
    # "writes zero assertion rows": a dismissal says the pair is not a steward task,
    # which is not a claim about whether the two records are the same person.
    assert assertion_count(initialised_lake) == before


def test_resolve_rollback_leaves_row_open(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC5: a rejected assertion exits 1 and leaves the row `open` — nothing partial.

    The failure is INJECTED, not inspected: an active `never` is asserted over the
    pair first, so S4.4's precedence rule refuses the `always` the resolution would
    write. That is the one failure S4.3.5's same-transaction requirement exists for,
    and the assertion row counts on both sides of the attempt are what prove nothing
    was written.
    """
    (row,) = seed_gray_band(initialised_lake, PAIR_MATCH)
    conflicting = run_er(
        "assert",
        "add",
        "--a",
        PAIR_MATCH[0],
        "--b",
        PAIR_MATCH[1],
        "--kind",
        NEVER,
        "--by",
        "tester",
    )
    assert conflicting.returncode == int(ExitCode.SUCCESS), conflicting.stdout + conflicting.stderr
    before = assertion_count(initialised_lake)

    refused = run_er(
        "review", "resolve", "--review-id", row.review_id, "--as", MATCH, "--by", "tester"
    )

    assert refused.returncode == int(ExitCode.STAGE_FAILURE), refused.stdout + refused.stderr
    # The refusal names the row that already holds the pair, because the operator's
    # next action is `er assert remove --assertion-id <that one>` (S4.4).
    assert "already active" in refused.stderr

    status, resolved_by, resolved_at = review_status(initialised_lake, row.review_id)
    assert status == OPEN
    assert resolved_by is None
    assert resolved_at is None
    assert assertion_count(initialised_lake) == before
    assert [assertion.kind for assertion in active_assertions(initialised_lake)] == [NEVER]


def test_review_list_exit_codes(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC6: `list` caps at --limit and exits 0; empty is 10; an unknown id is 2."""
    empty = run_er("review", "list", "--status", OPEN, "--json")

    assert empty.returncode == int(ExitCode.NOTHING_TO_DO), empty.stdout + empty.stderr
    assert stdout_records(empty) == []

    seed_gray_band(initialised_lake, PAIR_MATCH, PAIR_NO_MATCH, PAIR_DISMISS)

    listed = run_er("review", "list", "--status", OPEN, "--limit", "2", "--json")

    assert listed.returncode == int(ExitCode.SUCCESS), listed.stdout + listed.stderr
    records = stdout_records(listed)
    assert len(records) == 2
    for record in records:
        assert set(record) == {
            "review_id",
            "subject_type",
            "keys",
            "match_probability",
            "status",
        }
        assert record["status"] == OPEN
        assert record["subject_type"] == PAIR

    unknown = run_er(
        "review",
        "resolve",
        "--review-id",
        "01JQZ8XKQ4T7VN3M2B9CDEFGHX",
        "--as",
        MATCH,
        "--by",
        "tester",
    )
    assert unknown.returncode == int(ExitCode.CONFIG), unknown.stdout + unknown.stderr
    # Nothing was resolved by a refusal against a row that does not exist.
    assert (
        scalar(initialised_lake, f"SELECT count(*) FROM {REVIEW_QUEUE} WHERE status = ?", OPEN) == 3
    )


def test_entity_subject_resolution_rules(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC7: an S11 entity finding has no pair, so only `dismiss` is legal against it."""
    (row,) = upsert_entity_finding(
        initialised_lake, entity_id=ENTITY_ID, run_id=RUN_1, waterfall={"dispersion": 0.42}
    ).inserted
    assert (row.subject_type, row.reason) == (ENTITY, COHERENCE)
    assert (row.rec_a_key, row.rec_b_key) == (None, None)
    assert row.entity_id == ENTITY_ID

    refused = run_er(
        "review", "resolve", "--review-id", row.review_id, "--as", MATCH, "--by", "tester"
    )

    assert refused.returncode == int(ExitCode.CONFIG), refused.stdout + refused.stderr
    assert review_status(initialised_lake, row.review_id)[0] == OPEN
    assert assertion_count(initialised_lake) == 0

    dismissed = run_er(
        "review",
        "resolve",
        "--review-id",
        row.review_id,
        "--as",
        DISMISS,
        "--by",
        "tester",
        "--json",
    )

    assert dismissed.returncode == int(ExitCode.SUCCESS), dismissed.stdout + dismissed.stderr
    (record,) = stdout_records(dismissed)
    # The `keys` column of an entity subject is the entity it stands for: S4.0 gives
    # `er review` one `keys` field, and both subject types have to render into it.
    assert record["keys"] == ENTITY_ID
    assert review_status(initialised_lake, row.review_id)[0] == DISMISSED
    assert assertion_count(initialised_lake) == 0


def test_two_reasons_for_one_pair_keep_the_keys_tests_green(
    initialised_lake: duckdb.DuckDBPyConnection, dbt_packages: None
) -> None:
    """AC3: `dbt test --select tag:keys` is green with one pair open for two reasons.

    S5.0 puts `reason` in `review_queue`'s filtered uniqueness key precisely so this
    shape is legal (M20). A key that omitted it would fail here — which is the point:
    the claim is only observable once the relation holds two open rows for one
    canonical pair.
    """
    seed_gray_band(initialised_lake, PAIR_MATCH, PAIR_NO_MATCH)
    escalated = upsert_escalation(
        initialised_lake, rec_a_key=PAIR_MATCH[0], rec_b_key=PAIR_MATCH[1], run_id=RUN_1
    )
    assert escalated.added == 1
    reasons = initialised_lake.execute(
        f"SELECT reason, count(*) FROM {REVIEW_QUEUE} GROUP BY reason ORDER BY reason"
    ).fetchall()
    assert [(str(reason), int(count)) for reason, count in reasons] == [
        (GRAY_BAND, 2),
        (NEVER_UNSATISFIABLE, 1),
    ]

    with _detached(initialised_lake):
        completed = _dbt(
            "test", "--select", KEYS_SELECTOR, "--exclude", KEYS_EXCLUSION, "--target", "lake"
        )

    assert completed.returncode == 0, completed.stdout
    total = _TOTAL_RE.search(completed.stdout)
    assert total is not None, f"no summary line in dbt's output:\n{completed.stdout}"
    # A selector that matches nothing exits 0 and proves nothing (S12 M1).
    assert int(total.group(1)) > 0
