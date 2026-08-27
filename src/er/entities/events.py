"""`entity_events`: the replay log reconciliation writes (DesignDoc.md S5, S4.5.3-S4.5.5).

`entity_membership` is CURRENT STATE and holds no history; all of it lives here and
is folded in `(occurred_at, seq)` order (S4.5.3, D3). DuckLake has no sequences and
no non-literal `DEFAULT`s (S5.0), so every one of this relation's stamps is minted
in Python: `event_id` is a monotonic ULID from :mod:`er.entities.ids`, `seq` is
dense and 1-based per `run_id`, and `occurred_at` is supplied at flush time.

Three rules here are normative and are stated nowhere else in Python:

* **The idempotency key is `(run_id, entity_id, event_type, details_hash)`** and NOT
  `event_id` (S5.0, S4.5.4): "an event is written at most once per key; a re-run
  producing identical output writes zero events". :class:`EventLog` is where that
  collapse happens, which is why the accumulator and not the flush is the place a
  duplicate is detected.
* **`details_hash` has exactly one definition** — SHA-256 of
  :func:`canonical_details` — because the key above is only reproducible if two
  processes serialise the same document identically. The canonicalisation is S5.2's
  (sorted keys, no aliases, UTF-8, compact separators), the same one
  :func:`er.config.hashing.config_hash` uses.
* **No `VOLATILE_COLUMNS` member may appear inside `details`** (S5.0). `run_id`,
  `entity_id`'s stamps, `event_id`, `seq` and `occurred_at` are columns of the row;
  folding any of them into the hashed document would make `details_hash` differ
  between two runs that produced identical data, and every determinism comparison
  downstream of it would fail for the wrong reason.

Which events a reconcile emits is S4.5.3's overlap matrix and belongs to the
reconcile plan/apply stages; this module owns only their shape, their order and
their hash.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Iterable, Iterator, Mapping
from dataclasses import dataclass, field
from datetime import UTC, datetime
from types import MappingProxyType
from typing import Any, Final

import duckdb

from er.entities.ids import IdFactory, MonotonicUlidFactory, canonicalize_pair
from er.lake.model import EVENT_TYPES, REGISTRY, SCHEMA_QUALIFIER

__all__ = [
    "EVENT_DETAILS_SCHEMA",
    "EVENT_REASONS",
    "EVENT_TYPES",
    "MEMBER_REMOVED_CAUSES",
    "OPTIONAL_DETAIL_KEYS",
    "DetailsSchema",
    "Event",
    "EVENT_FOLD_VOCABULARY",
    "REPLAY_ORDER_COLUMNS",
    "EventLog",
    "InvalidEventDetailsError",
    "UnknownEventTypeError",
    "append_events",
    "canonical_details",
    "details_hash",
    "replay_membership",
]

_SPEC: Final = REGISTRY["entity_events"]

#: The relation's columns in S5 DDL order. Read from the TableSpec rather than
#: restated: :meth:`Event.row` is positional, so a column added to S5 must not be
#: able to shift a value into the wrong slot without this module failing.
EVENT_COLUMNS: Final[tuple[str, ...]] = _SPEC.column_names

_ENTITY_EVENTS: Final = f"{SCHEMA_QUALIFIER}.{_SPEC.name}"

#: Every event type accepts this key in addition to its required ones. S4.0: a
#: correction pass stamps `details.reason='correction_pass'` on every event it
#: emits, which is how a reader tells an INV-EQ repair from an ordinary change.
OPTIONAL_DETAIL_KEYS: Final[tuple[str, ...]] = ("reason",)

#: The only `reason` S4.0 names. A closed vocabulary because `reason` is hashed
#: into `details_hash`: a free-text field would make the idempotency key sensitive
#: to a caller's phrasing.
EVENT_REASONS: Final[frozenset[str]] = frozenset({"correction_pass"})

#: Why a record left its entity (S4.1.1, S4.5.5): `tombstone` for a deleted record,
#: `supersession` for one whose `content_hash` changed, `recluster` for a member the
#: new partition simply placed elsewhere.
MEMBER_REMOVED_CAUSES: Final[frozenset[str]] = frozenset({"tombstone", "supersession", "recluster"})

_NO_VOCABULARIES: Final[Mapping[str, frozenset[str]]] = MappingProxyType({"reason": EVENT_REASONS})


class UnknownEventTypeError(ValueError):
    """An `event_type` outside :data:`EVENT_TYPES` (S5).

    DuckLake enforces no `CHECK` and has no `ENUM` (S5.0), so this raise is the only
    thing between a typo and a row a dbt `accepted_values` test fails a whole run on.
    """

    def __init__(self, event_type: str) -> None:
        super().__init__(
            f"{event_type!r} is not an event type; S5 allows {', '.join(sorted(EVENT_TYPES))}"
        )
        self.event_type = event_type


class InvalidEventDetailsError(ValueError):
    """A `details` document that does not match its type's :data:`EVENT_DETAILS_SCHEMA` entry."""

    def __init__(self, event_type: str, problem: str) -> None:
        super().__init__(f"{event_type} details: {problem}")
        self.event_type = event_type


@dataclass(frozen=True, slots=True)
class DetailsSchema:
    """The shape of one event type's `details` document.

    Attributes:
        required: keys every document of this type MUST carry.
        record_key_lists: the subset of ``required`` whose value is a list of record
            keys. Stored ascending, so two orderings of the same membership change
            hash identically (S4.5.4, D2).
        pair_keys: the two keys holding a record pair, canonicalised through the one
            helper of S5.0/D9 so orientation cannot change the hash.
        vocabularies: closed value sets, by key. `reason` is in every entry's set
            because :data:`OPTIONAL_DETAIL_KEYS` is accepted everywhere.
    """

    required: tuple[str, ...]
    record_key_lists: tuple[str, ...] = ()
    pair_keys: tuple[str, str] | None = None
    vocabularies: Mapping[str, frozenset[str]] = field(default=_NO_VOCABULARIES)

    def __post_init__(self) -> None:
        allowed = set(self.required) | set(OPTIONAL_DETAIL_KEYS)
        for group in (self.record_key_lists, () if self.pair_keys is None else self.pair_keys):
            for key in group:
                if key not in self.required:
                    raise ValueError(f"{key!r} is not a required key of this schema")
        for key in self.vocabularies:
            if key not in allowed:
                raise ValueError(f"{key!r} has a vocabulary but is not an accepted key")

    @property
    def accepted(self) -> frozenset[str]:
        """Required keys plus the optional ones every type accepts."""
        return frozenset(self.required) | frozenset(OPTIONAL_DETAIL_KEYS)


def _schema(
    *required: str,
    record_key_lists: tuple[str, ...] = (),
    pair_keys: tuple[str, str] | None = None,
    vocabularies: Mapping[str, frozenset[str]] | None = None,
) -> DetailsSchema:
    """A :class:`DetailsSchema` with the always-accepted `reason` vocabulary folded in."""
    merged = dict(_NO_VOCABULARIES) | dict(vocabularies or {})
    return DetailsSchema(
        required=required,
        record_key_lists=record_key_lists,
        pair_keys=pair_keys,
        vocabularies=MappingProxyType(merged),
    )


#: Six of the seven types carry `member_keys`: the records the change moved. It is
#: the payload replay needs, because folding an event into `entity_membership`
#: (ER-080) is exactly adding or removing those keys on that entity.
_MEMBER_KEYS: Final = ("member_keys",)

#: One entry per member of :data:`EVENT_TYPES`, asserted exhaustive by the unit
#: tests rather than by inspection. The payloads:
#:
#: * `created` / `member_added` / `retired` — the members minted with, added to, or
#:   lost by the entity (S4.5.3). A `retired` entity holds zero members afterwards,
#:   so its list is the membership that departed.
#: * `member_removed` — the departing members and WHY they left (S4.1.1, S4.5.5).
#: * `merged` — emitted on the entity that lost all its members, naming the claimant
#:   it was merged into; `entities.merged_into` is the durable form of the same fact.
#: * `split` — emitted on a departing fragment, naming the entity it broke off from.
#: * `edge_cut` — the S4.4.2 partition-level `never` cut: the pair, its probability
#:   at cut time, the assertion the cut satisfies and the `cut_edges` row it wrote.
EVENT_DETAILS_SCHEMA: Final[Mapping[str, DetailsSchema]] = MappingProxyType(
    {
        "created": _schema("member_keys", record_key_lists=_MEMBER_KEYS),
        "member_added": _schema("member_keys", record_key_lists=_MEMBER_KEYS),
        "member_removed": _schema(
            "member_keys",
            "cause",
            record_key_lists=_MEMBER_KEYS,
            vocabularies={"cause": MEMBER_REMOVED_CAUSES},
        ),
        "merged": _schema("member_keys", "merged_into", record_key_lists=_MEMBER_KEYS),
        "split": _schema("member_keys", "split_from", record_key_lists=_MEMBER_KEYS),
        "retired": _schema("member_keys", record_key_lists=_MEMBER_KEYS),
        "edge_cut": _schema(
            "rec_a_key",
            "rec_b_key",
            "match_probability",
            "assertion_id",
            "cut_id",
            pair_keys=("rec_a_key", "rec_b_key"),
        ),
    }
)


def canonical_details(details: Mapping[str, Any]) -> str:
    """Return the canonical JSON serialisation :func:`details_hash` hashes.

    S5.2's rule, reused verbatim: keys sorted, no aliases, UTF-8, compact
    separators. ``sort_keys`` recurses, so a nested document is canonical too, and
    the result is what lands in the `details JSON` column — the stored document and
    the hashed one are the same bytes by construction.
    """
    return json.dumps(dict(details), sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def details_hash(details: Mapping[str, Any]) -> str:
    """SHA-256 hex digest of :func:`canonical_details`.

    THE implementation (S4.5.4): the idempotency key of `entity_events` is only
    reproducible across processes and runs if every writer and every reader hashes
    the same bytes.
    """
    return hashlib.sha256(canonical_details(details).encode("utf-8")).hexdigest()


def _record_key_list(event_type: str, key: str, value: Any) -> list[str]:
    """``value`` as an ascending list of record keys.

    Sorted here rather than at the call site because the sort is what makes AC5's
    property hold for every caller: two orderings of one membership change produce
    one `details_hash`, so a reconcile that visited its members in scan order cannot
    emit a second event for a change it already recorded.
    """
    if isinstance(value, str) or not isinstance(value, Iterable):
        raise InvalidEventDetailsError(event_type, f"{key} must be a list of record keys")
    items = list(value)
    for item in items:
        if not isinstance(item, str):
            raise InvalidEventDetailsError(event_type, f"{key} holds a non-string: {item!r}")
    return sorted(items)


def _validated_details(event_type: str, details: Mapping[str, Any]) -> Mapping[str, Any]:
    """Return ``details`` normalised for hashing, or raise.

    Normalisation is total: record-key lists are sorted, the pair of an `edge_cut` is
    canonicalised, and nothing else is touched. Validation is strict in both
    directions — a missing required key and an unknown key both raise — because
    `details` is the only place an event's payload lives and a silently dropped key
    is a replay that reconstructs the wrong membership.
    """
    schema = EVENT_DETAILS_SCHEMA.get(event_type)
    if schema is None:
        raise UnknownEventTypeError(event_type)

    supplied = frozenset(details)
    missing = sorted(frozenset(schema.required) - supplied)
    if missing:
        raise InvalidEventDetailsError(event_type, f"missing required key(s) {', '.join(missing)}")
    unknown = sorted(supplied - schema.accepted)
    if unknown:
        raise InvalidEventDetailsError(event_type, f"unknown key(s) {', '.join(unknown)}")

    normalised: dict[str, Any] = dict(details)
    for key in schema.record_key_lists:
        normalised[key] = _record_key_list(event_type, key, normalised[key])
    if schema.pair_keys is not None:
        a_key, b_key = schema.pair_keys
        a_value, b_value = normalised[a_key], normalised[b_key]
        if not isinstance(a_value, str) or not isinstance(b_value, str):
            raise InvalidEventDetailsError(event_type, f"{a_key} and {b_key} must be record keys")
        normalised[a_key], normalised[b_key] = canonicalize_pair(a_value, b_value)
    for key, vocabulary in schema.vocabularies.items():
        if key in normalised and normalised[key] not in vocabulary:
            raise InvalidEventDetailsError(
                event_type,
                f"{key}={normalised[key]!r} is not one of {', '.join(sorted(vocabulary))}",
            )
    return MappingProxyType(normalised)


@dataclass(frozen=True, slots=True)
class Event:
    """One accumulated `entity_events` row, minus its flush-time `occurred_at`.

    Built only by :meth:`EventLog.emit`, which is what guarantees ``details`` is
    normalised and ``details_hash`` is the digest of it.
    """

    event_id: str
    seq: int
    run_id: str
    entity_id: str
    event_type: str
    details: Mapping[str, Any]
    details_hash: str

    @property
    def idempotency_key(self) -> tuple[str, str, str, str]:
        """The S5.0 key an event is written at most once per."""
        return (self.run_id, self.entity_id, self.event_type, self.details_hash)

    def row(self, occurred_at: datetime) -> tuple[Any, ...]:
        """The row's values in :data:`EVENT_COLUMNS` order.

        The `details JSON` column takes the serialised document; DuckDB casts the
        VARCHAR, and the string it casts is the canonical one that was hashed.
        """
        return (
            self.event_id,
            self.seq,
            self.run_id,
            self.entity_id,
            self.event_type,
            canonical_details(self.details),
            self.details_hash,
            occurred_at,
        )


class EventLog:
    """One run's events, accumulated in memory and flushed once.

    A pure accumulator: it holds no connection and does no I/O, so reconciliation is
    testable as a function of its inputs plus the mint sequence (S4.5.4, D10). Two
    properties it maintains, both load-bearing:

    * `seq` is **dense and 1-based in emission order**. Dense because replay orders
      by `(occurred_at, seq)` and a gap is indistinguishable from a lost event;
      emission order rather than a sort afterwards because that IS the causal order
      a fold has to see.
    * a re-emission of an event whose `(run_id, entity_id, event_type,
      details_hash)` is already recorded is a **no-op** returning the recorded event
      — it mints no id and consumes no `seq` value (S4.5.4). A reconcile that
      reaches the same conclusion twice therefore writes one row, which is what
      makes "exactly one merged event" assertable.

    Args:
        run_id: the run every event of this log belongs to. One log per run: `seq`
            is per `run_id`, and the flush writes one run's rows.
        ids: id source, injected. Production mints monotonic ULIDs so `event_id`
            is a total order over the stream; tests pass
            :class:`~er.entities.ids.CountingIdFactory` for the same ids in every
            process.
    """

    __slots__ = ("_by_key", "_events", "_ids", "_run_id")

    def __init__(self, run_id: str, ids: IdFactory | None = None) -> None:
        self._run_id = run_id
        self._ids: IdFactory = MonotonicUlidFactory() if ids is None else ids
        self._events: list[Event] = []
        self._by_key: dict[tuple[str, str, str, str], Event] = {}

    @property
    def run_id(self) -> str:
        return self._run_id

    @property
    def events(self) -> tuple[Event, ...]:
        """The accumulated events in emission order, which is `seq` order."""
        return tuple(self._events)

    def __len__(self) -> int:
        return len(self._events)

    def __iter__(self) -> Iterator[Event]:
        return iter(self._events)

    def emit(
        self,
        entity_id: str,
        event_type: str,
        details: Mapping[str, Any] | None = None,
    ) -> Event:
        """Record one event, or return the identical one already recorded.

        Args:
            entity_id: the entity the event happened to.
            event_type: a member of :data:`EVENT_TYPES`.
            details: the type's :data:`EVENT_DETAILS_SCHEMA` document.

        Raises:
            UnknownEventTypeError: ``event_type`` is outside S5's vocabulary.
            InvalidEventDetailsError: ``details`` is missing a required key, carries
                an unknown one, or holds a value outside a closed vocabulary.
        """
        normalised = _validated_details(event_type, {} if details is None else details)
        digest = details_hash(normalised)
        key = (self._run_id, entity_id, event_type, digest)
        recorded = self._by_key.get(key)
        if recorded is not None:
            return recorded
        event = Event(
            event_id=self._ids.new(),
            seq=len(self._events) + 1,
            run_id=self._run_id,
            entity_id=entity_id,
            event_type=event_type,
            details=normalised,
            details_hash=digest,
        )
        self._events.append(event)
        self._by_key[key] = event
        return event

    def rows(self, occurred_at: datetime) -> tuple[tuple[Any, ...], ...]:
        """Every accumulated event as a row, in `seq` order."""
        return tuple(event.row(occurred_at) for event in self._events)


_COLUMN_LIST: Final = ", ".join(EVENT_COLUMNS)
_ROW_PLACEHOLDER: Final = f"({', '.join('?' for _ in EVENT_COLUMNS)})"
_INSERT_PREFIX: Final = f"INSERT INTO {_ENTITY_EVENTS} ({_COLUMN_LIST}) VALUES "


def append_events(
    connection: duckdb.DuckDBPyConnection,
    events: Iterable[Event],
    *,
    occurred_at: datetime | None = None,
) -> int:
    """Append ``events`` to `entity_events`, all of them in one statement.

    One statement, not one per row: a DuckLake snapshot is per statement, and
    S4.5.3 requires an event and the membership rewrite it describes to land in the
    same snapshot — a per-row flush would publish a half-written history.

    Deduplication is the accumulator's job, not this function's: :class:`EventLog`
    has already collapsed the run's idempotency-key duplicates, and a log is
    flushed once.

    Args:
        connection: an attached lake connection.
        events: accumulated events — an :class:`EventLog` is itself iterable.
        occurred_at: the stamp every appended row carries. Supplied by the writer
            because DuckLake has no `DEFAULT now()` (S5.0); one stamp for the whole
            flush, with `seq` breaking the tie in `(occurred_at, seq)` replay order.

    Returns:
        The number of rows appended.
    """
    # One stamp for the flush, sampled before the first row: `(occurred_at, seq)` is
    # the replay order, and a clock read per row would let two events of one run
    # differ in a column that carries no information (S4.5.3, S5.0).
    stamp = datetime.now(UTC).replace(tzinfo=None) if occurred_at is None else occurred_at
    rows = [event.row(stamp) for event in events]
    if not rows:
        return 0
    statement = _INSERT_PREFIX + ", ".join(_ROW_PLACEHOLDER for _ in rows)
    connection.execute(statement, [value for row in rows for value in row])
    return len(rows)


#: The replay order (S4.5.3), declared once. `entity_membership` is current state and
#: this pair is what turns the log back into it, so a reader that ordered by anything
#: else — `event_id`, physical row order — would fold a different history. Exported
#: because `tests/helpers/invariants.py` must not re-spell a VOLATILE_COLUMNS member as
#: a literal, and both of these are members of that set.
REPLAY_ORDER_COLUMNS: Final[tuple[str, str]] = ("occurred_at", "seq")


#: How each event type moves membership when the log is folded (S4.5.3, D3).
#:
#: `add` puts `details.member_keys` on the event's own entity; `remove` takes them
#: off it; `move` does both, taking them off the event's entity and putting them on
#: the entity named by the detail key beside the verb. `none` is membership-neutral
#: and is listed rather than omitted, because "this type does nothing" and "this type
#: is not handled" must not look the same to a reader or to :func:`replay_membership`.
#:
#: `merged` MOVES rather than merely removing: S4.5.3 emits it on the entity that lost
#: all of its members, and the members went somewhere. The destination is read from the
#: event's own `merged_into` detail and never from the `entities.merged_into` COLUMN —
#: D3's rule is that the fold depends on the log alone, and S5 describes that column as
#: the durable form of the same fact rather than as a second source of it.
EVENT_FOLD_VOCABULARY: Final[Mapping[str, tuple[str, str | None]]] = MappingProxyType(
    {
        "created": ("add", None),
        "member_added": ("add", None),
        "member_removed": ("remove", None),
        "merged": ("move", "merged_into"),
        "split": ("add", None),
        "retired": ("remove", None),
        "edge_cut": ("none", None),
    }
)


def replay_membership(
    events: Iterable[Mapping[str, Any]],
) -> dict[str, str]:
    """Fold `entity_events` into `record_key -> entity_id` (S4.5.3, D3, M3).

    This is the executable statement of what `entity_membership` MEANS relative to its
    log: current state is the fold of the history, so replaying every event must
    reproduce the table exactly. Nothing else in the system says that.

    Three properties are load-bearing:

    * **The sort is inside this function.** `(occurred_at, seq)` is the replay order
      (S4.5.3) and it is applied here, so a caller cannot make the answer depend on how
      it happened to query. A fold that trusted physical row order would agree with
      `entity_membership` on a small lake and diverge on a compacted one.
    * **An unknown `event_type` raises.** A type this fold skipped would make replay
      agree with membership *by doing less*, which is the one failure mode a
      replay-equals-table assertion cannot otherwise catch.
    * **Membership is a set per entity, not a scalar.** A split emits `created` and
      `split` on the same fragment carrying the same keys, and a merge can emit `merged`
      on the loser beside `member_added` on the claimant; applying either twice has to
      be harmless.

    Args:
        events: rows carrying at least `entity_id`, `event_type`, `occurred_at`, `seq`
            and `details` — a mapping, or the JSON text `entity_events.details` stores.

    Returns:
        `record_key -> entity_id` for every record the log leaves in an entity. A record
        whose last event removed it from everything is absent, which is what
        `entity_membership` holds for it too: no row.

    Raises:
        UnknownEventTypeError: an `event_type` outside :data:`EVENT_FOLD_VOCABULARY`.
    """
    holdings: dict[str, set[str]] = {}

    def members(entity_id: str) -> set[str]:
        return holdings.setdefault(entity_id, set())

    stamp, sequence = REPLAY_ORDER_COLUMNS
    for event in sorted(events, key=lambda row: (row[stamp], int(row[sequence]))):
        event_type = str(event["event_type"])
        if event_type not in EVENT_FOLD_VOCABULARY:
            raise UnknownEventTypeError(event_type)
        verb, destination_key = EVENT_FOLD_VOCABULARY[event_type]
        if verb == "none":
            continue

        raw = event["details"]
        details = raw if isinstance(raw, Mapping) else json.loads(str(raw))
        keys = {str(key) for key in details.get("member_keys", ())}
        entity_id = str(event["entity_id"])

        if verb == "add":
            members(entity_id).update(keys)
        elif verb == "remove":
            members(entity_id).difference_update(keys)
        else:
            members(entity_id).difference_update(keys)
            members(str(details[destination_key])).update(keys)

    return {record_key: entity_id for entity_id, keys in holdings.items() for record_key in keys}
