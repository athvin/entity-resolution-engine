"""The steward review queue: the S4.3.5 upsert, and resolution → assertion.

`review_queue` is the one relation three independent producers write to — the
S4.3.5 gray band, the S4.4.2 `never_unsatisfiable` escalation, and the S11
coherence scorer — and the whole point of the table is that a steward's answer
*sticks*. Four rules govern this module and are stated nowhere else in Python:

* **The write is an upsert with three outcomes, not two.** A new subject is
  INSERTed at `status='open'` with ``first_seen_run_id == last_seen_run_id``; an
  open subject has only its `last_seen_run_id` REFRESHED; and a subject already
  in :data:`RESOLVED_STATUSES` is SKIPPED — no insert, and *no refresh either*.
  Bumping a dismissed row's `last_seen_run_id` would let a dismissal decay into a
  row that looks freshly seen, which is the failure S4.3.5's skip clause exists
  to prevent.
* **`reason` is part of a row's identity, everywhere.** S5.0's filtered
  uniqueness key is ``(subject_type, rec_a_key, rec_b_key, entity_id, reason)``
  where ``status='open'``, because one pair can be open for two independent
  reasons and collapsing them would drop a steward task (M20). The insert /
  refresh / skip decision is therefore taken against that whole tuple: a pair
  whose `gray_band` row was dismissed can still be escalated as
  `never_unsatisfiable`, which is a different task and not a resurfacing of the
  dismissed one.
* **`waterfall` is retained, never projected.** S4.3.5 requires the `gamma_*`
  comparison vector *and* the per-comparison Bayes factors from `predict()`, so
  :func:`upsert_gray_band_pairs` refuses a payload carrying neither rather than
  storing a row a reviewer cannot act on.
* **Resolving to `match` / `no_match` writes the assertion (S4.3.5).** The
  assertion is ER-062's to write — :func:`~er.review.assertions.add_assertion`
  owns canonicalisation, precedence and the conflict refusal — and this module
  only decides *which* kind and orders the two writes. See
  :func:`resolve_review` for why the order is what it is.

The relation is `ddl.py`-owned (S5.0) and is created by `er init`; nothing here
issues DDL.
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, replace
from datetime import UTC, datetime
from typing import Any, Final

import duckdb

from er.entities.ids import (
    IdFactory,
    InvalidRecordKeyError,
    UlidFactory,
    canonicalize_pair,
    record_key,
    split_record_key,
)
from er.errors import ConfigError, StageFailure
from er.lake.model import (
    REGISTRY,
    REVIEW_REASONS,
    REVIEW_STATUSES,
    REVIEW_SUBJECT_TYPES,
    SCHEMA_QUALIFIER,
)
from er.review.assertions import ALWAYS, NEVER, Assertion, add_assertion

__all__ = [
    "COHERENCE",
    "DISMISS",
    "DISMISSED",
    "ENTITY",
    "GRAY_BAND",
    "MATCH",
    "NEVER_UNSATISFIABLE",
    "NO_MATCH",
    "OPEN",
    "PAIR",
    "RESOLUTIONS",
    "RESOLVED_MATCH",
    "RESOLVED_NO_MATCH",
    "RESOLVED_STATUSES",
    "REVIEW_QUEUE_RELATION",
    "REVIEW_REASONS",
    "REVIEW_STATUSES",
    "REVIEW_SUBJECT_TYPES",
    "GrayBandPair",
    "ReviewResolution",
    "ReviewRow",
    "UpsertResult",
    "open_reviews",
    "resolve_review",
    "upsert_entity_finding",
    "upsert_escalation",
    "upsert_gray_band_pairs",
]

#: The relation this module owns end to end (S5, `ddl.py`-owned).
REVIEW_QUEUE_RELATION: Final = "review_queue"

_REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.{REVIEW_QUEUE_RELATION}"

#: S5's column list, in DDL order, read from the one place that declares it.
_COLUMNS: Final[tuple[str, ...]] = REGISTRY[REVIEW_QUEUE_RELATION].column_names

#: `review_queue.subject_type` (S5). Both values are in :data:`REVIEW_SUBJECT_TYPES`.
PAIR: Final = "pair"
ENTITY: Final = "entity"

#: `review_queue.reason` (S4.3.5, S4.4.2, S11), spelled from the spec rather than
#: from a literal at each use. All three are in :data:`REVIEW_REASONS`.
GRAY_BAND: Final = "gray_band"
NEVER_UNSATISFIABLE: Final = "never_unsatisfiable"
COHERENCE: Final = "coherence"

#: `review_queue.status` (S5). All four are in :data:`REVIEW_STATUSES`.
OPEN: Final = "open"
RESOLVED_MATCH: Final = "resolved_match"
RESOLVED_NO_MATCH: Final = "resolved_no_match"
DISMISSED: Final = "dismissed"

#: The three statuses S4.3.5 skips. A subject in one of them is settled: it is not
#: re-inserted and its `last_seen_run_id` is not touched.
RESOLVED_STATUSES: Final[frozenset[str]] = frozenset({RESOLVED_MATCH, RESOLVED_NO_MATCH, DISMISSED})

#: S4.0's `er review resolve --as` vocabulary, in the order the flag's help lists it.
MATCH: Final = "match"
NO_MATCH: Final = "no_match"
DISMISS: Final = "dismiss"
RESOLUTIONS: Final[tuple[str, ...]] = (MATCH, NO_MATCH, DISMISS)

#: `--as` → the `status` the row lands in.
_RESOLUTION_STATUS: Final[Mapping[str, str]] = {
    MATCH: RESOLVED_MATCH,
    NO_MATCH: RESOLVED_NO_MATCH,
    DISMISS: DISMISSED,
}

#: `--as` → the `assertions.kind` S4.3.5 writes beside the resolution. `dismiss` is
#: absent deliberately: a dismissal asserts nothing about the pair.
_RESOLUTION_KIND: Final[Mapping[str, str]] = {MATCH: ALWAYS, NO_MATCH: NEVER}

#: The two key families a `gray_band` `waterfall` MUST carry (S4.3.5): Splink's
#: `predict()` emits one `gamma_<column>` per comparison and one `bf_<column>`
#: Bayes factor beside it, and S4.3.5 requires both to be retained rather than
#: projected away.
GAMMA_PREFIX: Final = "gamma_"
BAYES_FACTOR_PREFIX: Final = "bf_"


@dataclass(frozen=True)
class ReviewRow:
    """One `review_queue` row (S5), with `waterfall` already parsed.

    Frozen: a resolution produces a new value rather than mutating this one, so a
    caller cannot hold a row that still claims to be `open` after the UPDATE that
    resolved it.
    """

    review_id: str
    subject_type: str
    reason: str
    status: str
    first_seen_run_id: str
    last_seen_run_id: str
    rec_a_key: str | None = None
    rec_b_key: str | None = None
    entity_id: str | None = None
    match_probability: float | None = None
    waterfall: Mapping[str, Any] | None = None
    resolved_by: str | None = None
    resolved_at: datetime | None = None

    @property
    def pair(self) -> tuple[str, str] | None:
        """The canonical pair this row queues, or ``None`` for an entity subject."""
        if self.rec_a_key is None or self.rec_b_key is None:
            return None
        return (self.rec_a_key, self.rec_b_key)

    @property
    def keys(self) -> str:
        """S4.0's `keys` stdout column: the pair, or the entity it stands for.

        One field rather than three, because S4.0 gives `er review` five columns
        and a pair subject and an entity subject have to render into the same one.
        """
        pair = self.pair
        return f"{pair[0]} {pair[1]}" if pair is not None else str(self.entity_id)

    def manifest(self) -> dict[str, object]:
        """S4.0's stdout for `er review`: five fields, and no more."""
        return {
            "review_id": self.review_id,
            "subject_type": self.subject_type,
            "keys": self.keys,
            "match_probability": self.match_probability,
            "status": self.status,
        }

    def stdout_line(self) -> str:
        """:meth:`manifest` as the human summary S4.0's stdout column describes."""
        return ", ".join(f"{name}={_render(value)}" for name, value in self.manifest().items())


@dataclass(frozen=True)
class GrayBandPair:
    """One scored pair in the S4.3.5 gray band, as `er match` hands it over.

    ``waterfall`` is the `predict()` evidence verbatim — the same payload S5 puts
    in `match_scores.evidence`. It is passed whole rather than as a projection,
    because S4.3.5 requires the `gamma_*` vector and the per-comparison Bayes
    factors to be *retained*, and a caller free to choose the columns is a caller
    free to drop them.

    The pair is validated but NOT canonicalised here: canonicalisation is a
    write-time step through the single S5.0 helper, and doing it in the input type
    would put a second ordering site in front of it.
    """

    rec_a_key: str
    rec_b_key: str
    match_probability: float
    waterfall: Mapping[str, Any]


@dataclass(frozen=True)
class UpsertResult:
    """What one upsert pass did, in the terms S4.3.5's counters are stated in.

    Three outcomes, counted separately, because `review_queue_added` and
    `review_queue_refreshed` are distinct counters (S4.3.5) and because a *skipped*
    subject is the one outcome that must leave the relation byte-unchanged.
    """

    inserted: tuple[ReviewRow, ...] = ()
    refreshed: tuple[ReviewRow, ...] = ()
    skipped: tuple[ReviewRow, ...] = ()

    @property
    def added(self) -> int:
        """`review_queue_added` for this pass (S4.3.5, S4.5.6)."""
        return len(self.inserted)

    @property
    def refreshed_count(self) -> int:
        """`review_queue_refreshed` for this pass (S4.3.5)."""
        return len(self.refreshed)


@dataclass(frozen=True)
class ReviewResolution:
    """The outcome of `er review resolve`: the row, and the assertion it wrote.

    ``assertion`` is ``None`` for `dismiss`, which asserts nothing about the pair —
    that absence is the difference between "not a steward task" and "these two
    records are (not) the same person".
    """

    row: ReviewRow
    assertion: Assertion | None = None


@dataclass(frozen=True)
class _Subject:
    """One subject to upsert: its open-row key plus the payload a new row carries."""

    subject_type: str
    reason: str
    rec_a_key: str | None = None
    rec_b_key: str | None = None
    entity_id: str | None = None
    match_probability: float | None = None
    waterfall: Mapping[str, Any] | None = None


def _render(value: object) -> str:
    """A stdout field, with booleans in their JSON spelling and NULL as `\\N`."""
    if value is None:
        return "\\N"
    return str(value).lower() if isinstance(value, bool) else str(value)


def _stamp(moment: datetime | None) -> datetime:
    """``moment`` as the naive UTC value S5's `TIMESTAMP` columns hold.

    S5.0 gives every timestamp column `TIMESTAMP`, not `TIMESTAMPTZ`, so the offset
    is dropped at this boundary rather than left to an implicit engine cast.
    """
    stamped = datetime.now(UTC) if moment is None else moment
    if stamped.tzinfo is None:
        return stamped
    return stamped.astimezone(UTC).replace(tzinfo=None)


def _require_record_key(key: str) -> str:
    """``key`` if it is a well-formed S5.0 record key.

    :func:`~er.entities.ids.split_record_key` splits on the FIRST separator only, so
    ``crm:1:2`` would come back as ``('crm', '1:2')`` and look well formed. Feeding
    the halves back through :func:`~er.entities.ids.record_key` — which bans ``':'``
    in both components — is what rejects a key carrying a second separator.
    """
    return record_key(*split_record_key(key))


def _canonical_pair(a: str, b: str) -> tuple[str, str]:
    """The S5.0 canonical ordering of two record keys, or an S4.0 exit-`2` refusal.

    Raises:
        er.errors.ConfigError: either key is malformed, or the two are equal.
    """
    try:
        return canonicalize_pair(_require_record_key(a), _require_record_key(b))
    except (InvalidRecordKeyError, ValueError) as exc:
        raise ConfigError(f"malformed review pair ({a!r}, {b!r}): {exc}") from exc


def _require_waterfall(pair: GrayBandPair) -> Mapping[str, Any]:
    """``pair``'s waterfall, if it retains what S4.3.5 says it must.

    Raises:
        er.errors.StageFailure: the payload carries no `gamma_*` key or no
            per-comparison Bayes factor. S4.3.5 says both "MUST be retained rather
            than projected away", so a stripped payload is refused at the write
            rather than discovered by a steward looking at an empty waterfall.
    """
    payload = pair.waterfall
    missing = [
        prefix
        for prefix in (GAMMA_PREFIX, BAYES_FACTOR_PREFIX)
        if not any(str(key).startswith(prefix) for key in payload)
    ]
    if missing:
        raise StageFailure(
            f"gray-band waterfall for ({pair.rec_a_key!r}, {pair.rec_b_key!r}) carries no "
            f"{' and no '.join(f'{prefix}* key' for prefix in missing)}; S4.3.5 requires the "
            f"gamma_* comparison vector and the per-comparison Bayes factors to be retained"
        )
    return payload


_SELECT_SQL: Final = f"SELECT {', '.join(_COLUMNS)} FROM {_REVIEW_QUEUE}"

_INSERT_SQL: Final = (
    f"INSERT INTO {_REVIEW_QUEUE} ({', '.join(_COLUMNS)}) "
    f"VALUES ({', '.join('?' for _ in _COLUMNS)})"
)

_REFRESH_SQL: Final = f"UPDATE {_REVIEW_QUEUE} SET last_seen_run_id = ? WHERE review_id = ?"

_RESOLVE_SQL: Final = (
    f"UPDATE {_REVIEW_QUEUE} SET status = ?, resolved_by = ?, resolved_at = ? "
    f"WHERE review_id = ? AND status = '{OPEN}'"
)

# `IS NOT DISTINCT FROM` and not `=`: three of the five key columns are NULL on
# every row that does not use them — `entity_id` on a pair subject, both record
# keys on an entity subject — and `NULL = NULL` is NULL, so an `=` predicate would
# match no existing row and insert a duplicate on every run.
_SUBJECT_WHERE: Final = (
    "subject_type = ? AND reason = ? "
    "AND rec_a_key IS NOT DISTINCT FROM ? "
    "AND rec_b_key IS NOT DISTINCT FROM ? "
    "AND entity_id IS NOT DISTINCT FROM ?"
)


def _row_from(values: Sequence[Any]) -> ReviewRow:
    """One fetched tuple as a :class:`ReviewRow`, positionally against :data:`_COLUMNS`."""
    fields = dict(zip(_COLUMNS, values, strict=True))
    waterfall = fields["waterfall"]
    return ReviewRow(
        review_id=str(fields["review_id"]),
        subject_type=str(fields["subject_type"]),
        reason=str(fields["reason"]),
        status=str(fields["status"]),
        first_seen_run_id=str(fields["first_seen_run_id"]),
        last_seen_run_id=str(fields["last_seen_run_id"]),
        rec_a_key=None if fields["rec_a_key"] is None else str(fields["rec_a_key"]),
        rec_b_key=None if fields["rec_b_key"] is None else str(fields["rec_b_key"]),
        entity_id=None if fields["entity_id"] is None else str(fields["entity_id"]),
        match_probability=(
            None if fields["match_probability"] is None else float(fields["match_probability"])
        ),
        waterfall=None if waterfall is None else json.loads(str(waterfall)),
        resolved_by=None if fields["resolved_by"] is None else str(fields["resolved_by"]),
        resolved_at=fields["resolved_at"],
    )


def _insert(connection: duckdb.DuckDBPyConnection, row: ReviewRow) -> None:
    """Append one row, in S5's column order."""
    values: dict[str, Any] = {
        "review_id": row.review_id,
        "subject_type": row.subject_type,
        "rec_a_key": row.rec_a_key,
        "rec_b_key": row.rec_b_key,
        "entity_id": row.entity_id,
        "reason": row.reason,
        "match_probability": row.match_probability,
        # A JSON column takes the serialised document; DuckDB casts the VARCHAR.
        "waterfall": None if row.waterfall is None else json.dumps(dict(row.waterfall)),
        "status": row.status,
        "first_seen_run_id": row.first_seen_run_id,
        "last_seen_run_id": row.last_seen_run_id,
        "resolved_by": row.resolved_by,
        "resolved_at": row.resolved_at,
    }
    connection.execute(_INSERT_SQL, [values[column] for column in _COLUMNS])


def _rows_for(connection: duckdb.DuckDBPyConnection, subject: _Subject) -> list[ReviewRow]:
    """Every row carrying this subject's open-row key, resolved ones included."""
    rows = connection.execute(
        f"{_SELECT_SQL} WHERE {_SUBJECT_WHERE} ORDER BY review_id",
        [
            subject.subject_type,
            subject.reason,
            subject.rec_a_key,
            subject.rec_b_key,
            subject.entity_id,
        ],
    ).fetchall()
    return [_row_from(row) for row in rows]


def _by_id(connection: duckdb.DuckDBPyConnection, review_id: str) -> ReviewRow | None:
    """The row with this `review_id`, whatever its status."""
    rows = connection.execute(f"{_SELECT_SQL} WHERE review_id = ?", [review_id]).fetchall()
    return None if not rows else _row_from(rows[0])


def _upsert(
    connection: duckdb.DuckDBPyConnection,
    subjects: Sequence[_Subject],
    *,
    run_id: str,
    id_factory: IdFactory,
) -> UpsertResult:
    """Apply S4.3.5's insert / refresh / skip rule to each subject, in order.

    Raises:
        er.errors.StageFailure: a subject's open-row key carries more than one open
            row. S5.0's filtered uniqueness is a dbt test with no constraint behind
            it, so the invariant is this writer's to keep, and silently refreshing
            one of two would make the choice depend on scan order.
    """
    inserted: list[ReviewRow] = []
    refreshed: list[ReviewRow] = []
    skipped: list[ReviewRow] = []
    for subject in subjects:
        existing = _rows_for(connection, subject)
        settled = [row for row in existing if row.status in RESOLVED_STATUSES]
        if settled:
            # NOT refreshed: S4.3.5 skips a resolved subject outright, and bumping
            # `last_seen_run_id` here would let a dismissal decay into a row that
            # looks freshly seen.
            skipped.extend(settled)
            continue
        open_rows = [row for row in existing if row.status == OPEN]
        if len(open_rows) > 1:
            ids = ", ".join(row.review_id for row in open_rows)
            raise StageFailure(
                f"{REVIEW_QUEUE_RELATION} holds {len(open_rows)} open rows for "
                f"(subject_type={subject.subject_type!r}, rec_a_key={subject.rec_a_key!r}, "
                f"rec_b_key={subject.rec_b_key!r}, entity_id={subject.entity_id!r}, "
                f"reason={subject.reason!r}): {ids}. S5.0 permits one, and DuckLake "
                f"enforces no filtered uniqueness, so this is a writer that bypassed S4.3.5"
            )
        if open_rows:
            row = open_rows[0]
            connection.execute(_REFRESH_SQL, [run_id, row.review_id])
            refreshed.append(replace(row, last_seen_run_id=run_id))
            continue
        row = ReviewRow(
            review_id=id_factory.new(),
            subject_type=subject.subject_type,
            reason=subject.reason,
            status=OPEN,
            # Written once and never updated: `first_seen_run_id` is how long a
            # steward task has been waiting, and a refresh that moved it would
            # erase exactly that (S4.3.5).
            first_seen_run_id=run_id,
            last_seen_run_id=run_id,
            rec_a_key=subject.rec_a_key,
            rec_b_key=subject.rec_b_key,
            entity_id=subject.entity_id,
            match_probability=subject.match_probability,
            waterfall=subject.waterfall,
        )
        _insert(connection, row)
        inserted.append(row)
    return UpsertResult(
        inserted=tuple(inserted), refreshed=tuple(refreshed), skipped=tuple(skipped)
    )


def upsert_gray_band_pairs(
    connection: duckdb.DuckDBPyConnection,
    pairs: Iterable[GrayBandPair],
    *,
    run_id: str,
    id_factory: IdFactory | None = None,
) -> UpsertResult:
    """Queue the gray band of this run: `review_low <= p < auto_merge` (S4.3.5).

    THE write `er match` makes to `review_queue`; the band itself is the matching
    stage's to compute, and this function neither re-derives nor re-checks it.

    Args:
        connection: a connection with the lake attached, per S4.0b.
        pairs: the scored pairs in the band, in either key order.
        run_id: this run, for `first_seen_run_id` / `last_seen_run_id`.
        id_factory: the source of the minted `review_id`; production ULIDs by
            default, a counting factory in tests (S4.5.4, D10).

    Returns:
        What the pass inserted, refreshed and skipped — the three counts S4.3.5's
        `review_queue_added` / `review_queue_refreshed` counters are read from.

    Raises:
        er.errors.ConfigError: a malformed record key (S4.0 exit ``2``).
        er.errors.StageFailure: a waterfall stripped of its `gamma_*` vector or its
            Bayes factors (S4.3.5).
    """
    subjects: list[_Subject] = []
    for pair in pairs:
        waterfall = _require_waterfall(pair)
        rec_a_key, rec_b_key = _canonical_pair(pair.rec_a_key, pair.rec_b_key)
        subjects.append(
            _Subject(
                subject_type=PAIR,
                reason=GRAY_BAND,
                rec_a_key=rec_a_key,
                rec_b_key=rec_b_key,
                match_probability=pair.match_probability,
                waterfall=waterfall,
            )
        )
    return _upsert(
        connection,
        subjects,
        run_id=run_id,
        id_factory=id_factory if id_factory is not None else UlidFactory(),
    )


def upsert_escalation(
    connection: duckdb.DuckDBPyConnection,
    *,
    rec_a_key: str,
    rec_b_key: str,
    run_id: str,
    match_probability: float | None = None,
    waterfall: Mapping[str, Any] | None = None,
    id_factory: IdFactory | None = None,
) -> UpsertResult:
    """Queue an S4.4.2 `never_unsatisfiable` pair: every path between the endpoints
    is protected, so the pair is escalated rather than cut.

    It obeys the same insert / refresh / skip rule as the gray band, and it is a
    row of its own even when the pair is already open for `gray_band`: `reason` is
    in the open-row key precisely so the two steward tasks do not collapse (S5.0).

    No `waterfall` is required. An escalation is a *partition* finding, not a
    scored pair — S4.4.2 reaches it after clustering, from the cut search — so
    there is no `gamma_*` vector to retain, and S5 makes the column nullable.

    Raises:
        er.errors.ConfigError: a malformed record key (S4.0 exit ``2``).
    """
    canonical_a, canonical_b = _canonical_pair(rec_a_key, rec_b_key)
    return _upsert(
        connection,
        [
            _Subject(
                subject_type=PAIR,
                reason=NEVER_UNSATISFIABLE,
                rec_a_key=canonical_a,
                rec_b_key=canonical_b,
                match_probability=match_probability,
                waterfall=waterfall,
            )
        ],
        run_id=run_id,
        id_factory=id_factory if id_factory is not None else UlidFactory(),
    )


def upsert_entity_finding(
    connection: duckdb.DuckDBPyConnection,
    *,
    entity_id: str,
    run_id: str,
    waterfall: Mapping[str, Any] | None = None,
    id_factory: IdFactory | None = None,
) -> UpsertResult:
    """Queue an S11 coherence finding against a whole entity.

    `subject_type='entity'` with `entity_id` set and BOTH record keys NULL — the
    subject is the cluster, not a pair inside it — and `waterfall` carries the
    scorer's dispersion and outlier record keys (S11). The upsert rule is the same
    one the gray band obeys: refresh an open finding, skip a resolved one.

    Raises:
        er.errors.ConfigError: ``entity_id`` is empty. An entity subject with no
            entity is a row no steward can act on and no reader can key.
    """
    if not entity_id:
        raise ConfigError(
            f"a {ENTITY!r} review subject needs an entity_id; S5 makes it non-NULL "
            f"exactly when subject_type is {ENTITY!r}"
        )
    return _upsert(
        connection,
        [
            _Subject(
                subject_type=ENTITY,
                reason=COHERENCE,
                entity_id=entity_id,
                waterfall=waterfall,
            )
        ],
        run_id=run_id,
        id_factory=id_factory if id_factory is not None else UlidFactory(),
    )


def open_reviews(
    connection: duckdb.DuckDBPyConnection,
    *,
    status: str = OPEN,
    limit: int = 100,
) -> list[ReviewRow]:
    """The queue as `er review list [--status open] [--limit 100]` reports it (S4.0).

    Ordered by `review_id`, which is a ULID and therefore time-ordered: the oldest
    steward task is listed first, and `--limit` truncates the tail rather than an
    arbitrary slice of a scan.

    Raises:
        er.errors.ConfigError: ``status`` is outside :data:`REVIEW_STATUSES`, or
            ``limit`` is not positive. Both are S4.0 exit ``2``.
    """
    if status not in REVIEW_STATUSES:
        raise ConfigError(
            f"unknown review status {status!r}; S5 declares {', '.join(sorted(REVIEW_STATUSES))}"
        )
    if limit < 1:
        raise ConfigError(f"--limit must be at least 1, got {limit}")
    rows = connection.execute(
        f"{_SELECT_SQL} WHERE status = ? ORDER BY review_id LIMIT ?", [status, limit]
    ).fetchall()
    return [_row_from(row) for row in rows]


def resolve_review(
    connection: duckdb.DuckDBPyConnection,
    review_id: str,
    *,
    resolution: str,
    resolved_by: str,
    id_factory: IdFactory | None = None,
    resolved_at: datetime | None = None,
) -> ReviewResolution:
    """Resolve one row, writing the assertion S4.3.5 pairs with the resolution.

    `match` writes an ``always`` assertion for the row's canonical pair, `no_match`
    a ``never``, and `dismiss` writes none — a dismissal says "not a steward task",
    which is not a claim about the two records.

    **Write order, and why it is this one.** S4.3.5 wants the resolution and the
    assertion in one transaction, and the failure it is guarding against is a
    resolution that survives an assertion the S4.4 precedence rule rejected. The
    assertion is written FIRST, on this same connection, through
    :func:`~er.review.assertions.add_assertion` — which opens and commits its own
    transaction, and which DuckDB will not let this function nest inside another.
    Ordering it first is what makes the rejection path atomic *by construction*:
    :class:`~er.review.assertions.AssertionConflict` propagates with nothing
    written at all, so the row is still `open` with a NULL `resolved_at` and the
    `assertions` row count is unchanged. The residual — a status UPDATE that fails
    after the assertion committed — leaves the row `open` beside an active
    assertion, which re-running the same command repairs: `add_assertion` returns
    an identical active assertion without writing a second one, and the UPDATE then
    completes. The opposite order has no such repair, because a resolved row with
    no assertion looks finished.

    Args:
        connection: a connection with the lake attached, per S4.0b.
        review_id: the row to resolve.
        resolution: one of :data:`RESOLUTIONS`.
        resolved_by: the steward, for `resolved_by` and `assertions.created_by`.
        id_factory: the source of the minted `assertion_id` (S4.5.4, D10).
        resolved_at: the stamp for both writes; now, in UTC, when omitted.

    Returns:
        The row as it now stands, and the assertion written beside it, if any.

    Raises:
        er.errors.ConfigError: an unknown `review_id`, an unknown ``resolution``, a
            row that is not `open`, or `match`/`no_match` against an entity subject,
            which has no pair to assert over. Every one is S4.0 exit ``2``.
        er.review.assertions.AssertionConflict: the pair already carries an active
            assertion of the other kind (S4.4, exit ``1``). Nothing is written and
            the row stays `open`.
    """
    if resolution not in RESOLUTIONS:
        raise ConfigError(
            f"unknown resolution {resolution!r}; S4.0 declares {', '.join(RESOLUTIONS)}"
        )
    row = _by_id(connection, review_id)
    if row is None:
        raise ConfigError(
            f"{REVIEW_QUEUE_RELATION} holds no row for review_id={review_id!r}; "
            f"`er review list` is what enumerates the ids a steward can resolve"
        )
    if row.status != OPEN:
        raise ConfigError(
            f"review_id={review_id!r} is already {row.status!r}; S4.3.5 resolves an "
            f"open row, and a second resolution would rewrite when the steward decided"
        )
    stamped = _stamp(resolved_at)
    kind = _RESOLUTION_KIND.get(resolution)
    assertion: Assertion | None = None
    if kind is not None:
        pair = row.pair
        if row.subject_type != PAIR or pair is None:
            raise ConfigError(
                f"review_id={review_id!r} has subject_type={row.subject_type!r} and no pair, "
                f"so there is nothing to assert {kind!r} over; an entity subject is resolved "
                f"with --as {DISMISS} (S11)"
            )
        assertion = add_assertion(
            connection,
            a=pair[0],
            b=pair[1],
            kind=kind,
            created_by=resolved_by,
            # The durable link back to the steward task, so an operator reading
            # `assertions` can see which review produced the constraint.
            note=f"{REVIEW_QUEUE_RELATION}:{review_id}",
            id_factory=id_factory,
            created_at=stamped,
        )
    status = _RESOLUTION_STATUS[resolution]
    connection.execute("BEGIN TRANSACTION")
    try:
        connection.execute(_RESOLVE_SQL, [status, resolved_by, stamped, review_id])
    except BaseException:
        connection.execute("ROLLBACK")
        raise
    connection.execute("COMMIT")
    resolved = _by_id(connection, review_id)
    if resolved is None:
        # Unreachable: the row was read under this connection a few statements ago
        # and nothing here deletes. Stated rather than asserted so a caller never
        # receives a resolution describing a row that is not there.
        raise StageFailure(f"review_id={review_id!r} vanished during resolution")
    return ReviewResolution(row=resolved, assertion=assertion)
