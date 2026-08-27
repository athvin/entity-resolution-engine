"""The replay fold, over hand-built event lists (S4.5.3, D3, M3).

`entity_membership` is current state and `entity_events` is the whole history, so
"replay reproduces membership" is the one executable statement of how the two relate.
The integration arm makes that claim against real runs; this file pins the three
properties of the fold itself that a lake-backed test cannot isolate:

* **The sort is inside the function.** `(occurred_at, seq)` is the replay order, and a
  fold that trusted the order it was handed would agree with `entity_membership` on
  every test that happened to query with an `ORDER BY` and diverge the first time
  DuckLake compacted a table. Feeding it a shuffled list is the only way to see that.
* **An unknown `event_type` raises.** A skipped type makes replay agree with the table
  *by doing less* — the one failure a replay-equals-table assertion cannot catch on its
  own, because both sides would be missing the same change.
* **`edge_cut` is membership-neutral, deliberately.** It is in the event vocabulary and
  in the touched-set formula, so "this type does nothing" has to be a decision the code
  records rather than an omission that happens to work.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime
from typing import Any, Final

import pytest

from er.entities.events import (
    EVENT_FOLD_VOCABULARY,
    UnknownEventTypeError,
    replay_membership,
)
from er.lake.model import EVENT_TYPES

T0: Final = datetime(2026, 5, 1, tzinfo=UTC).replace(tzinfo=None)
T1: Final = datetime(2026, 5, 2, tzinfo=UTC).replace(tzinfo=None)

E1: Final = "01M0ENTITY0000000000000001"
E2: Final = "01M0ENTITY0000000000000002"


def event(
    entity_id: str,
    event_type: str,
    *,
    seq: int,
    occurred_at: datetime = T0,
    as_json: bool = False,
    **details: Any,
) -> dict[str, Any]:
    """One `entity_events` row as the fold reads it.

    ``as_json`` renders `details` as the VARCHAR the relation actually stores, which is
    what a caller reading straight out of DuckDB hands over — the fold has to accept
    both that and an already-parsed mapping.
    """
    payload: Any = json.dumps(details, sort_keys=True) if as_json else details
    return {
        "entity_id": entity_id,
        "event_type": event_type,
        "details": payload,
        "occurred_at": occurred_at,
        "seq": seq,
    }


def test_fold_vocabulary_covers_every_event_type() -> None:
    """A type S5 allows but the fold does not name would raise on real data."""
    assert set(EVENT_FOLD_VOCABULARY) == set(EVENT_TYPES), (
        f"the fold names {sorted(EVENT_FOLD_VOCABULARY)} but S5 allows "
        f"{sorted(EVENT_TYPES)}; every allowed type must have a decided verb"
    )


def test_fold_is_sorted_internally() -> None:
    """AC6: a shuffled list folds to the same mapping as an ordered one.

    The two orderings disagree about everything if the fold trusts its input: applied in
    order, `crm:C1` is created on E1 and then moves to E2; applied backwards, it is
    removed from an entity that never held it and then created on E1.
    """
    ordered = [
        event(E1, "created", seq=1, member_keys=["crm:C1", "crm:C2"]),
        event(E1, "member_removed", seq=2, member_keys=["crm:C1"], cause="recluster"),
        event(E2, "created", seq=3, member_keys=["crm:C1"]),
        event(E2, "member_added", seq=1, occurred_at=T1, member_keys=["crm:C3"]),
    ]
    expected = {"crm:C1": E2, "crm:C2": E1, "crm:C3": E2}
    assert replay_membership(ordered) == expected

    for rotation in range(1, len(ordered)):
        shuffled = ordered[rotation:] + ordered[:rotation]
        assert replay_membership(shuffled) == expected, (
            f"rotating the input by {rotation} changed the fold's answer, so the sort is "
            "outside the function and the caller's query order decides the result"
        )

    # Reversed as well as rotated: the pathological order, not merely a different one.
    assert replay_membership(list(reversed(ordered))) == expected


def test_unknown_event_type_raises() -> None:
    """AC2: a type outside the vocabulary is refused, never skipped."""
    with pytest.raises(UnknownEventTypeError) as raised:
        replay_membership([event(E1, "teleported", seq=1, member_keys=["crm:C1"])])
    assert "teleported" in str(raised.value)

    # And the raise is not a blanket refusal of anything unfamiliar-looking: every type
    # the vocabulary DOES name folds without complaint.
    for event_type in sorted(EVENT_FOLD_VOCABULARY):
        details: dict[str, Any] = {"member_keys": ["crm:C1"]}
        if event_type == "member_removed":
            details["cause"] = "recluster"
        elif event_type == "merged":
            details["merged_into"] = E2
        elif event_type == "split":
            details["split_from"] = E2
        elif event_type == "edge_cut":
            details = {
                "rec_a_key": "billing:B1",
                "rec_b_key": "crm:C1",
                "match_probability": 0.99,
                "assertion_id": "a",
                "cut_id": "c",
            }
        replay_membership([event(E1, event_type, seq=1, **details)])


def test_edge_cut_is_membership_neutral() -> None:
    """AC2: `edge_cut` folds without moving anybody."""
    before = [event(E1, "created", seq=1, member_keys=["crm:C1", "crm:C2"])]
    after = [
        *before,
        event(
            E1,
            "edge_cut",
            seq=2,
            rec_a_key="billing:B1",
            rec_b_key="crm:C1",
            match_probability=0.97,
            assertion_id="01M0A",
            cut_id="01M0C",
        ),
    ]
    assert replay_membership(after) == replay_membership(before), (
        "edge_cut changed the membership mapping; it records a severed EDGE, and the "
        "membership consequence of that cut arrives as its own split/member_removed "
        "events (S4.4.2)"
    )


def test_merged_moves_members_to_the_named_entity() -> None:
    """S4.5.3: `merged` is emitted on the entity that LOST its members.

    The members went somewhere, and the destination is read from the event's own
    `merged_into` detail. D3's rule is that the fold depends on the log alone — the
    `entities.merged_into` COLUMN is the durable form of the same fact, not a second
    source the fold is allowed to consult.
    """
    folded = replay_membership(
        [
            event(E1, "created", seq=1, member_keys=["crm:C1"]),
            event(E2, "created", seq=2, member_keys=["crm:C2"]),
            event(E1, "merged", seq=3, member_keys=["crm:C1"], merged_into=E2),
        ]
    )
    assert folded == {"crm:C1": E2, "crm:C2": E2}, (
        f"a merged entity's members did not land under the claimant: {folded}"
    )


def test_retired_entity_holds_nobody() -> None:
    """S4.5.3: a retired entity ends with zero members, so its records leave the map."""
    folded = replay_membership(
        [
            event(E1, "created", seq=1, member_keys=["crm:C1", "crm:C2"]),
            event(E1, "retired", seq=2, member_keys=["crm:C1", "crm:C2"]),
        ]
    )
    assert folded == {}, (
        f"a retired entity still holds {folded}; `entity_membership` holds no row for a "
        "record that left every entity, so the fold must not either"
    )


def test_details_may_arrive_as_stored_json() -> None:
    """`entity_events.details` is a VARCHAR; the fold accepts what the column holds."""
    as_mapping = replay_membership([event(E1, "created", seq=1, member_keys=["crm:C1"])])
    as_text = replay_membership([event(E1, "created", seq=1, as_json=True, member_keys=["crm:C1"])])
    assert as_mapping == as_text == {"crm:C1": E1}


def test_repeated_application_is_harmless() -> None:
    """Membership is a set per entity, so a doubled event changes nothing.

    Not a hypothetical: a split emits `created` AND `split` on the same fragment
    carrying the same `member_keys`, so the fold sees those keys added twice on every
    real split (measured on `assertions_scenario`).
    """
    folded = replay_membership(
        [
            event(E2, "created", seq=1, member_keys=["crm:C1"]),
            event(E2, "split", seq=2, member_keys=["crm:C1"], split_from=E1),
        ]
    )
    assert folded == {"crm:C1": E2}
