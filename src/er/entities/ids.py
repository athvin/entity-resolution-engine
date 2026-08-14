"""Identity primitives: record keys, pair canonicalisation, id minting, redirects.

Every later stage depends on this module, so it depends on nothing: it performs no
I/O, holds no connection, and imports no other module under ``src/er``. The lake
registry, config, matching and review packages all import it, and an import back
into any of them would close a cycle (S5.0, D6/D9/D10).

Four things are normative here and are stated nowhere else in Python:

* ``record_key`` is ``source_system || ':' || source_record_id`` and ``':'`` is
  banned inside the components (S5.0, D6).
* Pair canonicalisation happens in exactly ONE helper, at write time. Readers of
  ``match_scores`` / ``assertions`` / ``review_queue`` / ``cut_edges`` never do a
  two-sided join, so no other module may re-derive the ordering (S5.0, D9).
* All ids are ULIDs minted in Python — DuckLake has no sequences — and
  ``reconcile()`` takes an :class:`IdFactory` so it is a pure, reproducible
  function (S4.5.4, D10).
* ``entities.merged_into`` resolves EXTERNAL ids only, via :func:`resolve` with a
  cycle guard; it is never used to resolve current membership, which lives in
  ``entity_membership`` (S4.5.3, D3).
"""

from __future__ import annotations

import threading
import time
from collections.abc import Callable, Mapping
from typing import Protocol

from ulid import ULID, StrictMonotonicPolicy, ULIDGenerator

__all__ = [
    "RECORD_KEY_SEPARATOR",
    "CountingIdFactory",
    "IdCycleError",
    "IdFactory",
    "InvalidRecordKeyError",
    "MonotonicUlidFactory",
    "UlidFactory",
    "canonicalize_pair",
    "record_key",
    "resolve",
    "split_record_key",
]

#: S5.0: ``record_key VARCHAR := source_system || ':' || source_record_id``.
RECORD_KEY_SEPARATOR = ":"

#: Length of the canonical 26-character Crockford base32 ULID rendering. Every
#: factory here MUST produce this width: ids are compared and ORDER BY'd as
#: VARCHAR in the lake, and a short id would sort before a long one.
ULID_LENGTH = 26

#: Redirect chains in ``entities.merged_into`` are a handful of hops deep in
#: practice — an entity is merged, and its claimant may later be merged again.
#: The bound exists so a corrupt catalog fails fast instead of spinning; it is
#: generous enough that no healthy lake reaches it.
DEFAULT_MAX_HOPS = 64


class InvalidRecordKeyError(ValueError):
    """A record key, or one of its components, violates the S5.0 key model.

    Derived from :class:`ValueError` rather than from the ``src/er/errors.py``
    taxonomy because this module imports nothing else under ``src/er``.
    """


class IdCycleError(ValueError):
    """``entities.merged_into`` does not terminate for the id being resolved.

    Raised for a true cycle and for a chain longer than ``max_hops``; both mean
    the same thing to a caller — resolution cannot produce an answer.
    """


def record_key(source_system: str, source_record_id: str) -> str:
    """Return the canonical scalar record identity of S5.0.

    ``':'`` is banned in both components, not only in ``source_record_id`` as
    S5.0 states: :func:`split_record_key` splits on the FIRST separator, so a
    ``source_system`` containing one would not round-trip. The stricter rule is a
    superset of the spec's, and source systems are config-declared names (S6).
    """
    if not source_system:
        raise InvalidRecordKeyError("source_system must not be empty")
    if not source_record_id:
        raise InvalidRecordKeyError("source_record_id must not be empty")
    if RECORD_KEY_SEPARATOR in source_system:
        raise InvalidRecordKeyError(
            f"source_system must not contain {RECORD_KEY_SEPARATOR!r}: {source_system!r}"
        )
    if RECORD_KEY_SEPARATOR in source_record_id:
        raise InvalidRecordKeyError(
            f"source_record_id must not contain {RECORD_KEY_SEPARATOR!r}: {source_record_id!r}"
        )
    return f"{source_system}{RECORD_KEY_SEPARATOR}{source_record_id}"


def split_record_key(key: str) -> tuple[str, str]:
    """Inverse of :func:`record_key`, splitting on the first separator only."""
    source_system, separator, source_record_id = key.partition(RECORD_KEY_SEPARATOR)
    if not separator:
        raise InvalidRecordKeyError(
            f"record key has no {RECORD_KEY_SEPARATOR!r} separator: {key!r}"
        )
    if not source_system:
        raise InvalidRecordKeyError(f"record key has an empty source_system: {key!r}")
    if not source_record_id:
        raise InvalidRecordKeyError(f"record key has an empty source_record_id: {key!r}")
    return source_system, source_record_id


def canonicalize_pair(a: str, b: str) -> tuple[str, str]:
    """Return ``(rec_a_key, rec_b_key)`` with ``rec_a_key < rec_b_key`` lexically.

    THE canonicalisation helper of S5.0/D9. Every write to ``match_scores``,
    ``assertions``, ``review_queue`` (``subject_type='pair'``) and ``cut_edges``
    goes through it, which is what lets readers join one-sided and what makes the
    reconciliation determinism total order well defined.
    """
    if a == b:
        raise ValueError(f"a pair must have two distinct record keys, got {a!r} twice")
    return (a, b) if a < b else (b, a)


class IdFactory(Protocol):
    """Source of freshly minted 26-character ULID strings.

    Injected into ``reconcile()`` (production: ULID; tests: monotonic counter) so
    that reconciliation is a pure function of its inputs plus the mint sequence
    (S4.5.4, D10).
    """

    def new(self) -> str:
        """Mint one id."""


class UlidFactory:
    """Production factory: an independent ULID per call."""

    def new(self) -> str:
        return str(ULID())


def _system_millis() -> int:
    return time.time_ns() // 1_000_000


class _NonDecreasingClock:
    """Wrap a millisecond clock so it can never step backwards.

    ``StrictMonotonicPolicy`` only orders ids sharing a timestamp; a clock that
    regresses (NTP step, suspend/resume) would still emit a smaller ULID. The
    floor turns that into a stall at the previous millisecond, where the policy's
    randomness increment takes over.
    """

    def __init__(self, clock: Callable[[], int]) -> None:
        self._clock = clock
        self._floor = 0

    def __call__(self) -> int:
        self._floor = max(self._floor, self._clock())
        return self._floor


def _monotonic_generator(clock: Callable[[], int]) -> ULIDGenerator:
    return ULIDGenerator(clock=_NonDecreasingClock(clock), policy=StrictMonotonicPolicy())


# Strict monotonicity is a property of the id STREAM in a process, not of any one
# object, and `entity_events.event_id` is the relation's logical key whose replay
# order must be total (S5.0, MINOR-event_id). Instances therefore share this
# generator, so two factories built in one process cannot interleave into a
# descending sequence.
_SHARED_MONOTONIC_GENERATOR = _monotonic_generator(_system_millis)
_SHARED_MONOTONIC_LOCK = threading.Lock()


class MonotonicUlidFactory:
    """ULID factory whose output is strictly ascending, including within one millisecond.

    Args:
        clock: millisecond clock, for tests that need to freeze time. Supplying
            one also gives this instance a PRIVATE generator, so a frozen-clock
            test cannot drag the shared production stream back to 1970.
    """

    def __init__(self, clock: Callable[[], int] | None = None) -> None:
        if clock is None:
            self._generator = _SHARED_MONOTONIC_GENERATOR
            self._lock = _SHARED_MONOTONIC_LOCK
        else:
            self._generator = _monotonic_generator(clock)
            self._lock = threading.Lock()

    def new(self) -> str:
        # ULIDGenerator locks around the randomness policy but samples the clock
        # outside it, so the lock has to cover the whole call for the clock floor
        # to hold under concurrency.
        with self._lock:
            return str(self._generator.generate())


class CountingIdFactory:
    """Deterministic factory for tests: the same sequence in every process.

    Ids are the base32 rendering of an ascending integer, so they are valid
    26-character ULIDs and sort exactly as production ids do — a test that orders
    by id therefore exercises the real comparison. (Crockford base32 maps bytes to
    an ASCII-ascending alphabet at fixed width, so byte order IS lexical order.)
    """

    def __init__(self, start: int = 0) -> None:
        self._next = start

    def new(self) -> str:
        value = self._next
        self._next += 1
        return str(ULID.from_int(value))


def resolve(
    entity_id: str,
    redirects: Mapping[str, str],
    *,
    max_hops: int = DEFAULT_MAX_HOPS,
) -> str:
    """Follow ``entities.merged_into`` to the entity that absorbed ``entity_id``.

    ``redirects`` maps merged entity → claimant. An id with no redirect resolves
    to itself. This is the EXTERNAL id resolution path of S4.5.3 only: current
    membership is read from ``entity_membership``, never from here.

    Raises:
        IdCycleError: the chain revisits an id, or follows more than ``max_hops``
            redirects without reaching one.
    """
    # A list, not a set: the cycle it detects is also the diagnosis a steward
    # needs, and it is bounded by max_hops.
    seen = [entity_id]
    current = entity_id
    while (nxt := redirects.get(current)) is not None:
        if nxt in seen:
            cycle = " -> ".join([*seen, nxt])
            raise IdCycleError(f"redirect cycle in entities.merged_into: {cycle}")
        if len(seen) > max_hops:
            raise IdCycleError(
                f"redirect chain from {entity_id!r} did not terminate within "
                f"max_hops={max_hops} (reached {current!r})"
            )
        seen.append(nxt)
        current = nxt
    return current
