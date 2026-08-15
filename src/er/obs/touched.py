"""`er_touched_entities`: the run's touched set (DesignDoc.md S4.6, S5.2).

S4.6 computes the set as a formula over this run's `entity_events` and writes it
here **before** the marts run, so the marts join it on `run_id`. That indirection
is not a preference: passing the ULID list as a dbt `--vars` payload hard-fails
with `E2BIG` at scale, because tens of thousands of 26-character ULIDs exceed
Linux's 128 KB per-argv-element limit. dbt receives `--vars '{run_id: <ulid>}'`
and reads the set from this relation.

Two dispositions and no third (S5.2): `rebuild` entities are re-assembled by the
marts, and `retire` entities — merge losers, emptied split fragments, retired
entities — are deleted from `golden_records`, `golden_lineage` and `golden_display`
*after* the marts run, because `delete+insert` cannot remove a key that is absent
from the incoming batch.

Only the accessor ships here. Computing the set from `entity_events` and invoking
the marts is S4.6's assembly stage (ER-092), and the S11 coherence hook reads the
same two functions (ER-104).
"""

from __future__ import annotations

from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Final

import duckdb

from er.lake.model import DISPOSITIONS, SCHEMA_QUALIFIER

__all__ = [
    "DISPOSITIONS",
    "REBUILD",
    "RETIRE",
    "InvalidDispositionError",
    "TouchedEntity",
    "read_touched_entities",
    "write_touched_entities",
]

#: The two members of :data:`~er.lake.model.DISPOSITIONS`, named so a caller writes
#: ``REBUILD`` instead of a string literal that no checker can catch a typo in.
REBUILD: Final[str] = "rebuild"
RETIRE: Final[str] = "retire"

_TOUCHED: Final[str] = f"{SCHEMA_QUALIFIER}.er_touched_entities"


class InvalidDispositionError(ValueError):
    """A disposition outside `{rebuild, retire}` (S5.2).

    Raised before the first statement of a write, never partway through it: DuckLake
    enforces no `CHECK` (B2), so this function is the only thing standing between a
    typo and a marts join that silently reaps nothing.
    """

    def __init__(self, disposition: str) -> None:
        super().__init__(
            f"{disposition!r} is not a disposition; S5.2 allows {', '.join(sorted(DISPOSITIONS))}"
        )
        self.disposition = disposition


@dataclass(frozen=True, slots=True)
class TouchedEntity:
    """One row of the touched set: an entity and what is to be done with it."""

    entity_id: str
    disposition: str


def write_touched_entities(
    connection: duckdb.DuckDBPyConnection,
    run_id: str,
    entities: Iterable[tuple[str, str] | TouchedEntity],
) -> int:
    """Write ``entities`` as this run's touched set, idempotently.

    Idempotent on `(run_id, entity_id)`, which is the relation's logical key (S5.0):
    a second write of the same pair leaves exactly one row, carrying the second
    write's disposition. That matters because `assemble` is idempotent against
    `entity_id` (S4 preamble) and a re-executed stage must not double the set it
    hands the marts.

    Args:
        connection: an attached lake connection.
        run_id: the run whose set this is.
        entities: ``(entity_id, disposition)`` pairs, or :class:`TouchedEntity`
            values. Duplicated entity ids are collapsed, last write winning.

    Returns:
        The number of rows the run's set now holds for the supplied ids.

    Raises:
        InvalidDispositionError: a disposition outside `{rebuild, retire}`. Raised
            before anything is deleted or inserted.
    """
    rows: dict[str, str] = {}
    for entity in entities:
        touched = entity if isinstance(entity, TouchedEntity) else TouchedEntity(*entity)
        if touched.disposition not in DISPOSITIONS:
            raise InvalidDispositionError(touched.disposition)
        rows[touched.entity_id] = touched.disposition
    if not rows:
        return 0

    created_at = datetime.now(UTC).replace(tzinfo=None)
    keys: Sequence[str] = tuple(rows)
    placeholders = ",".join("?" for _ in keys)
    # Delete-then-insert rather than `MERGE INTO`: the whole row is rewritten, so
    # there is nothing to preserve, and the delete is what makes the second write
    # leave one row instead of two.
    connection.execute(
        f"DELETE FROM {_TOUCHED} WHERE run_id = ? AND entity_id IN ({placeholders})",
        [run_id, *keys],
    )
    connection.executemany(
        f"INSERT INTO {_TOUCHED} (run_id, entity_id, disposition, created_at) VALUES (?, ?, ?, ?)",
        [[run_id, entity_id, disposition, created_at] for entity_id, disposition in rows.items()],
    )
    return len(rows)


def read_touched_entities(
    connection: duckdb.DuckDBPyConnection,
    run_id: str,
    *,
    disposition: str | None = None,
) -> tuple[TouchedEntity, ...]:
    """This run's touched set, ordered by `entity_id`.

    Ordered because the reap step walks it and S4.5.4's determinism rule applies to
    everything downstream of it: an unordered read makes two identical runs emit
    their deletes in different orders.

    Args:
        connection: an attached lake connection.
        run_id: the run to read. Rows of every other run are excluded — the relation
            accumulates across runs, which is what makes the marts' `run_id` join the
            filter it is.
        disposition: restrict to one disposition, for the reap step's `retire` read.

    Raises:
        InvalidDispositionError: ``disposition`` is outside `{rebuild, retire}`.
    """
    if disposition is not None and disposition not in DISPOSITIONS:
        raise InvalidDispositionError(disposition)
    predicate = "run_id = ?"
    parameters: list[str] = [run_id]
    if disposition is not None:
        predicate += " AND disposition = ?"
        parameters.append(disposition)
    rows = connection.execute(
        f"SELECT entity_id, disposition FROM {_TOUCHED} WHERE {predicate} ORDER BY entity_id",
        parameters,
    ).fetchall()
    return tuple(TouchedEntity(str(entity_id), str(value)) for entity_id, value in rows)
