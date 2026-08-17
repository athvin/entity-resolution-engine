"""The `entity_events` writer, without Docker (S5, S5.0, S4.5.3-S4.5.5, S8.4).

What is asserted here is the four properties the rest of M3 is built on, each named
by the spec rather than by the implementation that happens to satisfy it:

* `details_hash` is a function of the DOCUMENT, not of a caller's key order, member
  ordering, pair orientation or JSON whitespace — and it is the same digest in a
  second process, because the idempotency key of S4.5.4 would otherwise hold only
  within one interpreter;
* `seq` is dense and 1-based in emission order, and a collapsed duplicate consumes
  no value from it, because replay orders by `(occurred_at, seq)` and a gap is
  indistinguishable from a lost event;
* every member of S5's `event_type` vocabulary has a `details` schema and every
  schema is closed in both directions, so a typo'd key cannot ride along unhashed;
* and no `VOLATILE_COLUMNS` member is reachable inside `details`, which is what
  keeps the D2 "byte-identical modulo minted identifiers" comparison meaningful.

`lake` is an in-memory database ATTACHed under the alias and holding S5's own
`entity_events`, as `tests/unit/review/test_queue_upsert.py` does it: every
statement in the module under test is written `lake.main.…` (S4.0b forbids DuckLake
being the default catalog), and a fixture that made the table reachable unqualified
would let a missing qualifier pass here and fail against a real lake.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from collections.abc import Iterator
from datetime import datetime
from itertools import pairwise
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest

from er.entities.events import (
    EVENT_COLUMNS,
    EVENT_DETAILS_SCHEMA,
    EVENT_TYPES,
    MEMBER_REMOVED_CAUSES,
    OPTIONAL_DETAIL_KEYS,
    Event,
    EventLog,
    InvalidEventDetailsError,
    UnknownEventTypeError,
    append_events,
    canonical_details,
    details_hash,
)
from er.entities.ids import CountingIdFactory, MonotonicUlidFactory
from er.lake.columns import VOLATILE_COLUMNS
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER, create_table_sql

REPO_ROOT = Path(__file__).resolve().parents[3]

ENTITY_EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"

RUN_1: Final = "01JQZ8XKQ4T7VN3M2B9CDEFGH1"
RUN_2: Final = "01JQZ8XKQ4T7VN3M2B9CDEFGH2"

#: One millisecond the clock never leaves, so an ascending `event_id` stream cannot
#: come from time (MINOR-`event_id`).
FROZEN_MILLIS: Final = 1_700_000_000_000

#: A stamp, not `now()`: `occurred_at` is a `VOLATILE_COLUMNS` member and every
#: assertion here that touches it must be able to vary it deliberately.
STAMP_1: Final = datetime(2026, 8, 17, 5, 30, 0)
STAMP_2: Final = datetime(2026, 8, 17, 6, 45, 12)

#: One event of every type in :data:`EVENT_TYPES`, in an order a reconcile could
#: plausibly emit them. Shared by the in-process and subprocess halves of the
#: cross-process hash assertion, so it is written JSON-serialisable.
EVENT_SEQUENCE: Final[list[list[Any]]] = [
    ["E1", "created", {"member_keys": ["crm:2", "billing:7"]}],
    ["E1", "member_added", {"member_keys": ["webforms:9"]}],
    ["E2", "member_removed", {"member_keys": ["crm:5"], "cause": "tombstone"}],
    ["E2", "retired", {"member_keys": ["crm:5"], "reason": "correction_pass"}],
    ["E3", "merged", {"member_keys": ["crm:1"], "merged_into": "E1"}],
    ["E4", "split", {"member_keys": ["billing:3"], "split_from": "E1"}],
    [
        "E1",
        "edge_cut",
        {
            "rec_a_key": "webforms:9",
            "rec_b_key": "crm:2",
            "match_probability": 0.42,
            "assertion_id": "01JQZ8XKQ4T7VN3M2B9CASSERT",
            "cut_id": "01JQZ8XKQ4T7VN3M2B9CUTCUT1",
        },
    ],
]

#: The same log, built in a fresh interpreter. Printed rather than pickled so the
#: comparison is over the bytes a second process would write to the lake.
HASH_SCRIPT: Final = """
import json, sys
from er.entities.events import EventLog
from er.entities.ids import CountingIdFactory

log = EventLog(sys.argv[1], CountingIdFactory(start=1))
for entity_id, event_type, details in json.loads(sys.argv[2]):
    log.emit(entity_id, event_type, details)
print(json.dumps([[e.event_id, e.seq, e.details_hash] for e in log]))
"""


def run_in_subprocess(script: str, *arguments: str) -> str:
    """Run `script` in a fresh interpreter against this working tree's `src/`."""
    env = dict(os.environ)
    env["PYTHONPATH"] = str(REPO_ROOT / "src")
    result = subprocess.run(
        [sys.executable, "-c", script, *arguments],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    return result.stdout


def build_log(run_id: str = RUN_1, start: int = 1) -> EventLog:
    """:data:`EVENT_SEQUENCE` accumulated under a reproducible id stream (S4.5.4, D10)."""
    log = EventLog(run_id, CountingIdFactory(start=start))
    for entity_id, event_type, details in EVENT_SEQUENCE:
        log.emit(entity_id, event_type, details)
    return log


def without(row: tuple[Any, ...], columns: frozenset[str]) -> tuple[Any, ...]:
    """``row`` with the named columns projected away, positionally against S5's order."""
    return tuple(
        value for column, value in zip(EVENT_COLUMNS, row, strict=True) if column not in columns
    )


@pytest.fixture
def lake() -> Iterator[duckdb.DuckDBPyConnection]:
    """An in-memory stand-in for the lake, holding S5's own `entity_events`."""
    connection = duckdb.connect()
    connection.execute("ATTACH ':memory:' AS lake")
    connection.execute(create_table_sql(REGISTRY["entity_events"]))
    try:
        yield connection
    finally:
        connection.close()


def test_details_hash_is_canonical_and_stable() -> None:
    ordered = {"member_keys": ["billing:7", "crm:2"], "merged_into": "E1"}
    reordered = {"merged_into": "E1", "member_keys": ["billing:7", "crm:2"]}
    assert details_hash(ordered) == details_hash(reordered)

    # "Whitespace in the input mapping" is whitespace in a rendering of it: an
    # indented document parses to the same mapping and MUST hash the same, which is
    # what makes a hand-edited fixture and a generated one comparable.
    indented = json.loads(json.dumps(ordered, indent=4, sort_keys=False))
    assert details_hash(indented) == details_hash(ordered)
    assert canonical_details(ordered) == '{"member_keys":["billing:7","crm:2"],"merged_into":"E1"}'

    for changed in (
        {"member_keys": ["billing:7", "crm:3"], "merged_into": "E1"},
        {"member_keys": ["billing:7", "crm:2"], "merged_into": "E2"},
        {"member_keys": ["billing:7"], "merged_into": "E1"},
    ):
        assert details_hash(changed) != details_hash(ordered)

    # Across processes: the idempotency key of S4.5.4 spans runs, so a digest that
    # depended on this interpreter's dict iteration or hash seed would let a re-run
    # append a duplicate of every event.
    here = [[event.event_id, event.seq, event.details_hash] for event in build_log()]
    there = json.loads(run_in_subprocess(HASH_SCRIPT, RUN_1, json.dumps(EVENT_SEQUENCE)))
    assert here == there


def test_seq_is_dense_and_one_based() -> None:
    log = build_log()
    assert [event.seq for event in log] == list(range(1, len(EVENT_SEQUENCE) + 1))
    assert len(log) == len(EVENT_SEQUENCE)
    assert [event.event_type for event in log] == [row[1] for row in EVENT_SEQUENCE]

    # Frozen clock: `event_id` ascends because the factory is strictly monotonic,
    # not because time passed between two emissions (MINOR-`event_id`).
    frozen = EventLog(RUN_1, MonotonicUlidFactory(clock=lambda: FROZEN_MILLIS))
    for entity_id, event_type, details in EVENT_SEQUENCE:
        frozen.emit(entity_id, event_type, details)
    ids = [event.event_id for event in frozen]
    assert all(len(event_id) == 26 for event_id in ids)
    assert all(earlier < later for earlier, later in pairwise(ids))
    assert [event.seq for event in frozen] == list(range(1, len(EVENT_SEQUENCE) + 1))


def test_duplicate_idempotency_key_is_a_noop() -> None:
    log = EventLog(RUN_1, CountingIdFactory(start=1))
    first = log.emit("E1", "created", {"member_keys": ["crm:2", "billing:7"]})
    assert first.idempotency_key == (RUN_1, "E1", "created", first.details_hash)

    # The same event reached by a different route: the members in the other order,
    # which is exactly what a reconcile that scanned its input differently produces.
    again = log.emit("E1", "created", {"member_keys": ["billing:7", "crm:2"]})
    assert again is first
    assert len(log) == 1
    assert log.events == (first,)

    # The collapse consumed neither an id nor a `seq` value: the next distinct event
    # is `seq` 2, so the run's sequence stays dense (S4.5.4).
    following = log.emit("E1", "member_added", {"member_keys": ["webforms:9"]})
    assert following.seq == 2
    assert following.event_id > first.event_id

    # `run_id` is part of the key, so the same event under a second run is a second
    # row — the key is per run, and re-running is what mints a new `run_id`.
    other_run = EventLog(RUN_2, CountingIdFactory(start=1))
    twin = other_run.emit("E1", "created", {"member_keys": ["crm:2", "billing:7"]})
    assert twin.details_hash == first.details_hash
    assert twin.idempotency_key != first.idempotency_key


def test_details_schema_covers_every_event_type() -> None:
    assert EVENT_TYPES == frozenset(
        {"created", "member_added", "member_removed", "merged", "split", "retired", "edge_cut"}
    )
    # One definition, not two: the writer's vocabulary IS the domain the TableSpec
    # declares for `entity_events.event_type`, which is what a dbt `accepted_values`
    # test is generated from (S5.0).
    assert EVENT_TYPES == REGISTRY["entity_events"].enums["event_type"]
    assert frozenset(EVENT_DETAILS_SCHEMA) == EVENT_TYPES

    log = EventLog(RUN_1, CountingIdFactory(start=1))
    with pytest.raises(UnknownEventTypeError):
        log.emit("E1", "unmerged", {"member_keys": []})

    with pytest.raises(InvalidEventDetailsError) as missing:
        log.emit("E1", "member_removed", {"member_keys": ["crm:2"]})
    assert "cause" in str(missing.value)

    with pytest.raises(InvalidEventDetailsError) as unknown:
        log.emit("E1", "created", {"member_keys": ["crm:2"], "membre_keys": ["crm:3"]})
    assert "membre_keys" in str(unknown.value)

    # Closed vocabularies: `cause` is S4.5.5's three, and `reason` is S4.0's one.
    with pytest.raises(InvalidEventDetailsError):
        log.emit("E1", "member_removed", {"member_keys": ["crm:2"], "cause": "deleted"})
    with pytest.raises(InvalidEventDetailsError):
        log.emit("E1", "created", {"member_keys": ["crm:2"], "reason": "because"})
    for cause in sorted(MEMBER_REMOVED_CAUSES):
        accepted = log.emit("E9", "member_removed", {"member_keys": ["crm:2"], "cause": cause})
        assert accepted.details["cause"] == cause

    # `reason` is accepted by every type, since a correction pass stamps it on every
    # event it emits (S4.0).
    assert OPTIONAL_DETAIL_KEYS == ("reason",)
    for event_type, schema in EVENT_DETAILS_SCHEMA.items():
        assert schema.required, f"{event_type} has no required key"
        assert "reason" in schema.accepted


def test_record_key_lists_are_sorted() -> None:
    log = EventLog(RUN_1, CountingIdFactory(start=1))
    descending = log.emit("E1", "created", {"member_keys": ["webforms:9", "crm:2", "billing:7"]})
    assert list(descending.details["member_keys"]) == ["billing:7", "crm:2", "webforms:9"]

    ascending = EventLog(RUN_1, CountingIdFactory(start=1)).emit(
        "E1", "created", {"member_keys": ["billing:7", "crm:2", "webforms:9"]}
    )
    assert ascending.details_hash == descending.details_hash

    # The pair inside an `edge_cut` obeys the same rule through the one
    # canonicalisation helper of S5.0/D9: `rec_a_key < rec_b_key`, whichever way the
    # caller passed the endpoints.
    payload: dict[str, Any] = dict(EVENT_SEQUENCE[6][2])
    swapped = dict(payload, rec_a_key=payload["rec_b_key"], rec_b_key=payload["rec_a_key"])
    cut = log.emit("E1", "edge_cut", payload)
    assert (cut.details["rec_a_key"], cut.details["rec_b_key"]) == ("crm:2", "webforms:9")
    assert log.emit("E1", "edge_cut", swapped) is cut

    with pytest.raises(InvalidEventDetailsError):
        log.emit("E1", "created", {"member_keys": "crm:2"})
    with pytest.raises(InvalidEventDetailsError):
        log.emit("E1", "created", {"member_keys": ["crm:2", 7]})


def test_no_volatile_column_inside_details() -> None:
    for event_type, schema in EVENT_DETAILS_SCHEMA.items():
        overlap = schema.accepted & VOLATILE_COLUMNS
        assert not overlap, f"{event_type} details would carry volatile column(s) {overlap}"

    # And it is unreachable at runtime, not merely absent from the schema: the row's
    # own stamps are unknown keys, so a caller cannot fold one into the hash.
    log = EventLog(RUN_1, CountingIdFactory(start=1))
    for volatile in sorted(VOLATILE_COLUMNS):
        with pytest.raises(InvalidEventDetailsError):
            log.emit("E1", "created", {"member_keys": ["crm:2"], volatile: "x"})


def test_output_is_byte_identical_modulo_minted_ids() -> None:
    minted = frozenset({"event_id", "occurred_at"})
    # Both are `VOLATILE_COLUMNS` members, which is why D2 excludes them; the set is
    # imported rather than restated (S5.0).
    assert minted <= VOLATILE_COLUMNS

    first = build_log().rows(STAMP_1)
    second = build_log().rows(STAMP_2)
    assert [without(row, minted) for row in first] == [without(row, minted) for row in second]

    # Under the counting factory the ids match too, so the only difference two runs
    # of the same input can have is the stamp.
    assert [without(row, frozenset({"occurred_at"})) for row in first] == [
        without(row, frozenset({"occurred_at"})) for row in second
    ]
    assert all(row[EVENT_COLUMNS.index("occurred_at")] == STAMP_1 for row in first)

    # The `details` column carries the canonical document, so "byte-identical" is a
    # claim about the bytes that reach the lake and not only about the Python values.
    details_column = EVENT_COLUMNS.index("details")
    assert [row[details_column] for row in first] == [row[details_column] for row in second]
    assert json.loads(first[0][details_column]) == {"member_keys": ["billing:7", "crm:2"]}


def test_append_events_writes_one_row_per_event(lake: duckdb.DuckDBPyConnection) -> None:
    log = build_log()
    assert append_events(lake, log, occurred_at=STAMP_1) == len(EVENT_SEQUENCE)

    rows = lake.execute(
        f"SELECT {', '.join(EVENT_COLUMNS)} FROM {ENTITY_EVENTS} ORDER BY seq"
    ).fetchall()
    assert [tuple(row) for row in rows] == list(log.rows(STAMP_1))

    # An empty flush touches nothing: a reconcile that changed no entity emits no
    # events and must not open a snapshot for them (S4.5.4).
    assert append_events(lake, (), occurred_at=STAMP_1) == 0
    assert lake.execute(f"SELECT count(*) FROM {ENTITY_EVENTS}").fetchone() == (len(rows),)

    # The stamp defaults to the writer's clock, since DuckLake has no `DEFAULT now()`.
    solo: Event = EventLog(RUN_2, CountingIdFactory(start=1)).emit(
        "E5", "retired", {"member_keys": []}
    )
    assert append_events(lake, [solo]) == 1
    stamped = lake.execute(
        f"SELECT occurred_at FROM {ENTITY_EVENTS} WHERE run_id = ?", [RUN_2]
    ).fetchone()
    assert stamped is not None and stamped[0] > STAMP_1
