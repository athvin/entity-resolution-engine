"""The nickname lexicon: the `nickname_variants` relation and its S4.0 verbs.

S4.2's `name_variants(col)` expands names through this relation, which makes the
lexicon a *matching input that no `content_hash` covers*: editing it changes
standardized output while every record's raw bytes stay identical, which is
exactly the drift class a `std_version` bump names. Three properties close that
hole and are stated nowhere else:

* **Rows are retractable, never deleted** — the `assertions` shape. `remove`
  flips ``active`` and stamps ``retracted_by``/``retracted_at``, so lexicon
  history is auditable and a retracted pair can be re-added as a new row.
* **The active set has one hash.** :func:`lexicon_hash` digests the canonicalised
  active pairs; `runs.lexicon_hash` records it, and the S4.0 drift guard compares
  the two like any other fingerprint field. An orientation duplicate — ``(a, b)``
  active beside ``(b, a)`` — is the same *semantic* lexicon, so pairs are
  canonicalised (``min, max``) before hashing AND before writing, and the
  filtered logical key of S5.0 can therefore hold.
* **Seeding is `er init`'s job** (S4.0): the packaged CSV loads only into an
  empty relation, and rows created by the pre-lexicon dbt seed — recognisable by
  their NULL ``variant_id`` — are backfilled into the retractable shape rather
  than re-inserted beside themselves.

Names are normalized to the same lowercase form `name_norm` produces before they
are compared or stored; a pair whose two names collapse to one value is refused
(exit ``2``) because the macro's one-hop walk would expand it into nothing.
"""

from __future__ import annotations

import csv
import hashlib
from dataclasses import dataclass
from datetime import UTC, datetime
from importlib import resources
from pathlib import Path

import duckdb

from er.entities.ids import IdFactory, UlidFactory
from er.errors import ConfigError
from er.lake.model import SCHEMA_QUALIFIER

__all__ = [
    "LEXICON_RELATION",
    "SEED_CREATED_BY",
    "LexiconPair",
    "active_pairs",
    "add_pair",
    "lexicon_hash",
    "load_pairs",
    "packaged_seed_pairs",
    "remove_pair",
    "seed_lexicon",
]

LEXICON_RELATION = "nickname_variants"

#: ``created_by`` of every row `er init` seeds from the packaged CSV.
SEED_CREATED_BY = "seed"

#: The packaged starter lexicon, shipped beside this module.
_SEED_RESOURCE = "nickname_variants.csv"

_QUALIFIED = f"{SCHEMA_QUALIFIER}.{LEXICON_RELATION}"

# The pair separator inside one hashed unit and between units. The same two
# control characters `std_hash` uses for `name_variants` (S8.3 T-STD-1), chosen
# there because no normalized name can contain them.
_UNIT_SEP = "\x1e"
_RECORD_SEP = "\x1f"


@dataclass(frozen=True, slots=True)
class LexiconPair:
    """One written or retracted row, in the S4.0 output shape."""

    variant_id: str
    variant_a: str
    variant_b: str
    active: bool


def _normalize(name: str, flag: str) -> str:
    """The lowercase form the macro compares against, or the exit-``2`` refusal."""
    normalized = name.strip().lower()
    if not normalized:
        raise ConfigError(f"er lexicon: {flag} must be a non-empty name")
    return normalized


def _canonical(a: str, b: str) -> tuple[str, str]:
    """The stored orientation: ``variant_a < variant_b`` lexically (S5.0's shape)."""
    first = _normalize(a, "--a")
    second = _normalize(b, "--b")
    if first == second:
        raise ConfigError(
            f"er lexicon: {first!r} cannot be its own variant; the pair must "
            f"name two distinct normalized names"
        )
    return (first, second) if first < second else (second, first)


def _relation_present(connection: duckdb.DuckDBPyConnection) -> bool:
    row = connection.execute(
        "SELECT count(*) FROM duckdb_tables() WHERE database_name = ? AND table_name = ?",
        [SCHEMA_QUALIFIER.split(".", 1)[0], LEXICON_RELATION],
    ).fetchone()
    return row is not None and int(row[0]) > 0


def active_pairs(connection: duckdb.DuckDBPyConnection) -> tuple[tuple[str, str], ...]:
    """The active pairs, canonicalised and sorted — the hashed set, readable."""
    rows = connection.execute(
        f"SELECT variant_a, variant_b FROM {_QUALIFIED} WHERE active"
    ).fetchall()
    return tuple(
        sorted({(a, b) if a < b else (b, a) for a, b in ((str(x), str(y)) for x, y in rows)})
    )


def lexicon_hash(connection: duckdb.DuckDBPyConnection) -> str | None:
    """SHA-256 over the canonical active pairs; ``None`` before `er init`.

    A pure function of the semantic lexicon: orientation duplicates collapse, the
    order is total, and an empty (but existing) relation hashes to the digest of
    the empty string rather than to ``None`` — absence of the relation and
    absence of pairs are different states and the guard must not conflate them.
    """
    if not _relation_present(connection):
        return None
    pairs = active_pairs(connection)
    joined = _RECORD_SEP.join(f"{a}{_UNIT_SEP}{b}" for a, b in pairs)
    return hashlib.sha256(joined.encode("utf-8")).hexdigest()


def packaged_seed_pairs() -> tuple[tuple[str, str], ...]:
    """The packaged CSV's pairs, canonicalised, in file order."""
    text = (resources.files("er.std") / _SEED_RESOURCE).read_text(encoding="utf-8")
    reader = csv.DictReader(text.splitlines())
    return tuple(_canonical(row["variant_a"], row["variant_b"]) for row in reader)


def _now() -> datetime:
    # Naive UTC, like every S5 `TIMESTAMP` writer (er.review.assertions._stamp).
    return datetime.now(UTC).replace(tzinfo=None)


def _insert(
    connection: duckdb.DuckDBPyConnection,
    pair: tuple[str, str],
    *,
    created_by: str,
    factory: IdFactory,
) -> LexiconPair:
    variant_id = factory.new()
    connection.execute(
        f"INSERT INTO {_QUALIFIED} (variant_id, variant_a, variant_b, active, "
        f"created_by, created_at, retracted_by, retracted_at) "
        f"VALUES (?, ?, ?, TRUE, ?, ?, NULL, NULL)",
        [variant_id, pair[0], pair[1], created_by, _now()],
    )
    return LexiconPair(variant_id, pair[0], pair[1], True)


def _active_row_id(connection: duckdb.DuckDBPyConnection, pair: tuple[str, str]) -> str | None:
    """The active row for the pair in either orientation, if one exists."""
    row = connection.execute(
        f"SELECT variant_id FROM {_QUALIFIED} WHERE active AND "
        f"((variant_a = ? AND variant_b = ?) OR (variant_a = ? AND variant_b = ?))",
        [pair[0], pair[1], pair[1], pair[0]],
    ).fetchone()
    return None if row is None else str(row[0])


def add_pair(
    connection: duckdb.DuckDBPyConnection,
    a: str,
    b: str,
    *,
    created_by: str,
    id_factory: IdFactory | None = None,
) -> LexiconPair | None:
    """Activate one pair; ``None`` when it is already active (exit ``10``)."""
    pair = _canonical(a, b)
    if _active_row_id(connection, pair) is not None:
        return None
    return _insert(connection, pair, created_by=created_by, factory=id_factory or UlidFactory())


def remove_pair(
    connection: duckdb.DuckDBPyConnection, a: str, b: str, *, retracted_by: str
) -> LexiconPair | None:
    """Retract one pair; ``None`` when no active row carries it (exit ``10``)."""
    pair = _canonical(a, b)
    variant_id = _active_row_id(connection, pair)
    if variant_id is None:
        return None
    connection.execute(
        f"UPDATE {_QUALIFIED} SET active = FALSE, retracted_by = ?, retracted_at = ? "
        f"WHERE variant_id = ?",
        [retracted_by, _now(), variant_id],
    )
    return LexiconPair(variant_id, pair[0], pair[1], False)


def load_pairs(
    connection: duckdb.DuckDBPyConnection,
    path: Path,
    *,
    created_by: str,
    id_factory: IdFactory | None = None,
) -> tuple[LexiconPair, ...]:
    """Bulk-activate a CSV of ``variant_a,variant_b`` pairs; idempotent.

    Returns only the pairs this call activated — an empty tuple when every row
    of the file was already active, which the CLI reports as exit ``10``
    (the `er assert load` convention).
    """
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ConfigError(f"er lexicon load: cannot read {path}: {exc}") from exc
    reader = csv.DictReader(text.splitlines())
    if reader.fieldnames is None or {"variant_a", "variant_b"} - set(reader.fieldnames):
        raise ConfigError("er lexicon load: the file must carry a variant_a,variant_b header")
    factory = id_factory or UlidFactory()
    written: list[LexiconPair] = []
    seen: set[tuple[str, str]] = set()
    for row in reader:
        pair = _canonical(row["variant_a"], row["variant_b"])
        if pair in seen or _active_row_id(connection, pair) is not None:
            continue
        seen.add(pair)
        written.append(_insert(connection, pair, created_by=created_by, factory=factory))
    return tuple(written)


def seed_lexicon(
    connection: duckdb.DuckDBPyConnection, *, id_factory: IdFactory | None = None
) -> int:
    """`er init`'s seeding pass: backfill pre-lexicon rows, then seed if empty.

    Two idempotent halves, in this order:

    * Rows created by the pre-lexicon dbt seed carry NULL in every retractable
      column. They are backfilled — minted ``variant_id``, ``active=TRUE``,
      ``created_by='seed'`` — never re-inserted, so a migrated lake keeps its
      byte-identical `name_variants` arrays.
    * A relation with no rows at all receives the packaged CSV.

    Returns the number of rows written or backfilled; ``0`` on the second and
    every later invocation, and ``0`` when the relation does not exist yet — the
    writer window calls this before `er init` has necessarily created it.
    """
    if not _relation_present(connection):
        return 0
    factory = id_factory or UlidFactory()
    touched = 0
    orphans = connection.execute(
        f"SELECT variant_a, variant_b FROM {_QUALIFIED} WHERE variant_id IS NULL"
    ).fetchall()
    for a, b in orphans:
        connection.execute(
            f"UPDATE {_QUALIFIED} SET variant_id = ?, active = TRUE, created_by = ?, "
            f"created_at = ? WHERE variant_a = ? AND variant_b = ? AND variant_id IS NULL",
            [factory.new(), SEED_CREATED_BY, _now(), a, b],
        )
        touched += 1
    row = connection.execute(f"SELECT count(*) FROM {_QUALIFIED}").fetchone()
    if row is not None and int(row[0]) == 0:
        for pair in packaged_seed_pairs():
            _insert(connection, pair, created_by=SEED_CREATED_BY, factory=factory)
            touched += 1
    return touched
