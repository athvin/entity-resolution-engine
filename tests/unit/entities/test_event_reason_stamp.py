"""The rebuild-reason stamp of S4.0/S5.1, at the event layer (S4.5.4).

`details.reason` participates in `details_hash` — S4.5.4's idempotency key — so a
stamped and an unstamped emission of otherwise-identical details are two DIFFERENT
events. That consequence is the load-bearing one and it is asserted here directly,
because every downstream claim (a rebuild's events coexist with an ordinary run's;
a reason cannot be smuggled in as free text) follows from it.
"""

from __future__ import annotations

import pytest

from er.entities.events import EVENT_REASONS, EventLog, details_hash, stamp_reason
from er.entities.ids import CountingIdFactory
from er.entities.reconcile import RebuildReason
from er.lake.model import REBUILD_REASONS

RUN_ID = "01TESTRUN0000000000000000"
DETAILS = {"member_keys": ["s:a", "s:b"]}


def test_reason_is_part_of_details_hash() -> None:
    """Stamping changes the idempotency key, and the log emits the stamped form."""
    stamped = stamp_reason(DETAILS, "correction_pass")
    assert stamped["reason"] == "correction_pass"
    assert stamped["member_keys"] == DETAILS["member_keys"]
    # The hash is over the canonical serialisation, so the added key MUST move it —
    # this is what lets a rebuild's account of an entity coexist with an ordinary
    # run's instead of deduplicating against it (S4.5.4).
    assert details_hash(stamped) != details_hash(DETAILS)

    log = EventLog(RUN_ID, ids=CountingIdFactory(), reason="correction_pass")
    event = log.emit("entity-1", "created", DETAILS)
    assert event.details["reason"] == "correction_pass"
    assert event.details_hash == details_hash(stamp_reason(DETAILS, "correction_pass"))

    # Re-emitting the identical stamped event is the usual no-op (S4.5.4).
    again = log.emit("entity-1", "created", DETAILS)
    assert again is event
    assert len(log) == 1

    bare = EventLog(RUN_ID, ids=CountingIdFactory())
    unstamped = bare.emit("entity-1", "created", DETAILS)
    assert "reason" not in unstamped.details
    assert unstamped.details_hash != event.details_hash

    # Stamping the same reason twice is a no-op; a DIFFERENT reason is a conflict,
    # because one run has one reason (S5.1).
    assert stamp_reason(stamped, "correction_pass") == stamped
    with pytest.raises(ValueError, match="one run has one reason"):
        stamp_reason(stamped, "operator")


def test_unknown_reason_rejected() -> None:
    """The vocabulary is closed at every entry point, and it IS S5.1's."""
    with pytest.raises(ValueError, match="not a rebuild reason"):
        stamp_reason(DETAILS, "because")
    with pytest.raises(ValueError, match="not a rebuild reason"):
        EventLog(RUN_ID, reason="because")

    # One vocabulary, three spellings: the runs-row enum, the event stamp's
    # accepted set, and the typed CLI surface must be the same four values.
    assert EVENT_REASONS == REBUILD_REASONS
    assert {member.value for member in RebuildReason} == REBUILD_REASONS
