"""Steward assertions: the S4.4 lifecycle, precedence, and CONTRADICTION-1.

Assertions are what make a steward's correction survive a re-run, so almost
everything here is about *durability of the record* rather than about clustering.
Four rules govern the module and are stated nowhere else in Python:

* **No code path deletes a row.** :func:`retract_assertion` sets ``active=false``
  and stamps ``retracted_by``/``retracted_at``; the row stays. The assertion delta
  between two runs (S4.5.1) is computed from those stamps, and a deleted row would
  make the delta silently smaller rather than visibly wrong.
* **Every write canonicalises through :func:`er.entities.ids.canonicalize_pair`.**
  A caller passing ``--a`` greater than ``--b`` gets a canonicalised row, not an
  error, so ``rec_a_key < rec_b_key`` holds on every row and readers never join
  two-sided (S5.0, D9). There is no second ordering implementation in this package.
* **Precedence is enforced at WRITE time.** ``never`` dominates ``always`` for the
  same pair, and S4.4 rejects a conflicting insert (exit ``1``) rather than
  ordering around it — so at most one ``active`` row per pair exists and no reader
  has to break a tie. :func:`check_contradiction_1` is the opposite kind of thing:
  a pure function over a sequence of assertions that returns findings and fails
  nothing. Wiring its verdict into `er reconcile` is ER-074's.
* **Assertions are referred to by canonical pair, never by ULID.** The S8.2.1
  header is literal and carries no ``assertion_id`` column, which is exactly what
  keeps a committed fixture stable across runs; :func:`assertion_id_for` is the
  resolution step a caller needs when it wants the minted id of a pair it knows.

The relation is `ddl.py`-owned (S5.0) and is created by `er init`; nothing here
issues DDL.
"""

from __future__ import annotations

import csv
import io
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, replace
from datetime import UTC, datetime
from pathlib import Path
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
from er.lake.model import ASSERTION_KINDS, REGISTRY, SCHEMA_QUALIFIER

__all__ = [
    "ALWAYS",
    "ASSERTIONS_CSV_COLUMNS",
    "ASSERTIONS_CSV_HEADER",
    "ASSERTIONS_RELATION",
    "NEVER",
    "NULL_TOKEN",
    "PHASES",
    "Assertion",
    "AssertionConflict",
    "AssertionInput",
    "Contradiction",
    "active_assertions",
    "add_assertion",
    "assertion_id_for",
    "check_contradiction_1",
    "load_assertions_csv",
    "parse_assertions_csv",
    "retract_assertion",
]

#: The relation this module owns end to end (S5, `ddl.py`-owned).
ASSERTIONS_RELATION: Final = "assertions"

_ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.{ASSERTIONS_RELATION}"

#: S5's column list, in DDL order, read from the one place that declares it.
_COLUMNS: Final[tuple[str, ...]] = REGISTRY[ASSERTIONS_RELATION].column_names

#: `assertions.kind`, spelled from S4.4 rather than from a literal at each use.
#: Both values are in :data:`er.lake.model.ASSERTION_KINDS`.
ALWAYS: Final = "always"
NEVER: Final = "never"

#: S8.2.1's phase vocabulary, which the `phase` column of `assertions.csv` is drawn
#: from and from no other. `tests/helpers/scenario.py` states the same tuple for the
#: fixture linter; `src/er` may not import the test tree, and this is the pipeline
#: side of one normative list rather than a second vocabulary.
PHASES: Final[tuple[str, ...]] = ("base", "batch", "refresh", "resurrect")

#: S8.2.1's `assertions.csv` header. LITERAL: the loader compares the first line
#: byte for byte, because a fixture whose header drifted would otherwise be read
#: positionally and land a `phase` value in `rec_a_key`.
ASSERTIONS_CSV_COLUMNS: Final[tuple[str, ...]] = (
    "phase",
    "rec_a_key",
    "rec_b_key",
    "kind",
    "created_by",
    "note",
)
ASSERTIONS_CSV_HEADER: Final = ",".join(ASSERTIONS_CSV_COLUMNS)

#: S8.2.1's null token. An EMPTY field is the empty string, which is a distinct
#: value, so the two are mapped to `NULL` and `''` respectively and never conflated.
NULL_TOKEN: Final = "\\N"


class AssertionConflict(StageFailure):
    """A pair already carries an ``active`` assertion of the other kind (S4.4).

    S4.4 rejects the insert at write time — exit ``1`` — rather than ordering
    around it, so the offending row is carried on the exception: an operator's next
    action is `er assert remove --assertion-id <the existing one>`, and a message
    that did not name it would leave them querying the lake to find out which.
    """

    def __init__(self, existing: Assertion, kind: str) -> None:
        super().__init__(
            f"cannot add a {kind!r} assertion for "
            f"({existing.rec_a_key!r}, {existing.rec_b_key!r}): assertion_id="
            f"{existing.assertion_id!r} is already active with kind={existing.kind!r}. "
            f"S4.4 rejects a conflicting insert; retract that row first"
        )
        self.existing = existing
        self.kind = kind


@dataclass(frozen=True)
class Assertion:
    """One `assertions` row (S5), with `active` already a bool.

    Frozen: a retraction produces a new value rather than mutating this one, which
    is what stops a caller holding a row that claims to be active after the UPDATE
    that retracted it.
    """

    assertion_id: str
    rec_a_key: str
    rec_b_key: str
    kind: str
    active: bool
    created_by: str
    created_at: datetime
    retracted_by: str | None = None
    retracted_at: datetime | None = None
    note: str | None = None

    @property
    def pair(self) -> tuple[str, str]:
        """The canonical pair this row constrains."""
        return (self.rec_a_key, self.rec_b_key)

    def manifest(self) -> dict[str, object]:
        """S4.0's stdout for `er assert`: five fields, and no more."""
        return {
            "assertion_id": self.assertion_id,
            "rec_a_key": self.rec_a_key,
            "rec_b_key": self.rec_b_key,
            "kind": self.kind,
            "active": self.active,
        }

    def stdout_line(self) -> str:
        """:meth:`manifest` as the human summary S4.0's stdout column describes."""
        return ", ".join(f"{name}={_render(value)}" for name, value in self.manifest().items())


@dataclass(frozen=True)
class AssertionInput:
    """One parsed `assertions.csv` row (S8.2.1), still carrying its `phase`.

    The phase is retained rather than consumed here: `er assert load` applies every
    row in the file, and slicing a scenario's assertions to the phase about to run
    is the caller's job (S8.2.1, S8.3).

    The pair is validated but NOT canonicalised. Canonicalisation is a write-time
    step through the single helper of S5.0, and doing it here would put a second
    ordering site in front of it.
    """

    phase: str
    rec_a_key: str
    rec_b_key: str
    kind: str
    created_by: str
    note: str | None


@dataclass(frozen=True)
class Contradiction:
    """One CONTRADICTION-1 finding: a `never` pair inside one always-component (S4.4.1).

    The constraint set is unsatisfiable as a partition and no cutting strategy can
    repair it, so this is what a hard pre-clustering failure is reported with —
    every offending `assertion_id` plus the closure component S4.4.1 requires in
    `run_stages.error_detail`.
    """

    rec_a_key: str
    rec_b_key: str
    never_assertion_id: str
    always_assertion_ids: tuple[str, ...]
    component: frozenset[str]

    @property
    def assertion_ids(self) -> tuple[str, ...]:
        """Every offending id: the `never`, then the `always` edges that close over it."""
        return (self.never_assertion_id, *self.always_assertion_ids)

    def detail(self) -> str:
        """The S4.4.1 diagnosis, as it is written to `run_stages.error_detail`."""
        return (
            f"CONTRADICTION-1: never({self.rec_a_key}, {self.rec_b_key}) lies inside the "
            f"always-closure component {{{', '.join(sorted(self.component))}}}; "
            f"assertion_ids={', '.join(self.assertion_ids)}"
        )


def _render(value: object) -> str:
    """A stdout field, with booleans in their JSON spelling rather than Python's."""
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

    Raises:
        er.entities.ids.InvalidRecordKeyError: the key violates the S5.0 key model.
    """
    return record_key(*split_record_key(key))


def _canonical_pair(a: str, b: str) -> tuple[str, str]:
    """The S5.0 canonical ordering of two record keys, or an S4.0 exit-`2` refusal.

    Raises:
        er.errors.ConfigError: either key is malformed, or the two are equal. S4.0
            puts "malformed key" in the exit-`2` row of `er assert`.
    """
    try:
        return canonicalize_pair(_require_record_key(a), _require_record_key(b))
    except (InvalidRecordKeyError, ValueError) as exc:
        raise ConfigError(f"malformed assertion pair ({a!r}, {b!r}): {exc}") from exc


def _require_kind(kind: str) -> str:
    """``kind`` if S4.4 declares it.

    Raises:
        er.errors.ConfigError: an unknown kind, which S4.0 gives exit ``2``.
    """
    if kind not in ASSERTION_KINDS:
        raise ConfigError(
            f"unknown assertion kind {kind!r}; S4.4 declares {', '.join(sorted(ASSERTION_KINDS))}"
        )
    return kind


_SELECT_SQL: Final = f"SELECT {', '.join(_COLUMNS)} FROM {_ASSERTIONS}"

_INSERT_SQL: Final = (
    f"INSERT INTO {_ASSERTIONS} ({', '.join(_COLUMNS)}) VALUES ({', '.join('?' for _ in _COLUMNS)})"
)

_RETRACT_SQL: Final = (
    f"UPDATE {_ASSERTIONS} SET active = false, retracted_by = ?, retracted_at = ? "
    f"WHERE assertion_id = ? AND active"
)


def _row_from(values: Sequence[Any]) -> Assertion:
    """One fetched tuple as an :class:`Assertion`, positionally against :data:`_COLUMNS`."""
    fields = dict(zip(_COLUMNS, values, strict=True))
    return Assertion(
        assertion_id=str(fields["assertion_id"]),
        rec_a_key=str(fields["rec_a_key"]),
        rec_b_key=str(fields["rec_b_key"]),
        kind=str(fields["kind"]),
        active=bool(fields["active"]),
        created_by=str(fields["created_by"]),
        created_at=fields["created_at"],
        retracted_by=None if fields["retracted_by"] is None else str(fields["retracted_by"]),
        retracted_at=fields["retracted_at"],
        note=None if fields["note"] is None else str(fields["note"]),
    )


def _insert(connection: duckdb.DuckDBPyConnection, assertion: Assertion) -> None:
    """Append one row, in S5's column order."""
    values: dict[str, Any] = {
        "assertion_id": assertion.assertion_id,
        "rec_a_key": assertion.rec_a_key,
        "rec_b_key": assertion.rec_b_key,
        "kind": assertion.kind,
        "active": assertion.active,
        "created_by": assertion.created_by,
        "created_at": assertion.created_at,
        "retracted_by": assertion.retracted_by,
        "retracted_at": assertion.retracted_at,
        "note": assertion.note,
    }
    connection.execute(_INSERT_SQL, [values[column] for column in _COLUMNS])


def _active_for_pair(
    connection: duckdb.DuckDBPyConnection, rec_a_key: str, rec_b_key: str
) -> Assertion | None:
    """The one ``active`` row for a CANONICAL pair, or ``None``.

    Raises:
        er.errors.StageFailure: the pair carries more than one active row. S5.0's
            filtered uniqueness is a dbt test with no constraint behind it, so the
            invariant is this writer's to keep, and silently picking one of two
            would make precedence depend on scan order.
    """
    rows = connection.execute(
        f"{_SELECT_SQL} WHERE rec_a_key = ? AND rec_b_key = ? AND active ORDER BY assertion_id",
        [rec_a_key, rec_b_key],
    ).fetchall()
    if not rows:
        return None
    if len(rows) > 1:
        ids = ", ".join(str(row[0]) for row in rows)
        raise StageFailure(
            f"{ASSERTIONS_RELATION} holds {len(rows)} active rows for "
            f"({rec_a_key!r}, {rec_b_key!r}): {ids}. S5.0 permits one, and DuckLake "
            f"enforces no filtered uniqueness, so this is a writer that bypassed S4.4"
        )
    return _row_from(rows[0])


def _by_id(connection: duckdb.DuckDBPyConnection, assertion_id: str) -> Assertion | None:
    """The row with this `assertion_id`, active or not."""
    rows = connection.execute(f"{_SELECT_SQL} WHERE assertion_id = ?", [assertion_id]).fetchall()
    return None if not rows else _row_from(rows[0])


def _add(
    connection: duckdb.DuckDBPyConnection,
    *,
    a: str,
    b: str,
    kind: str,
    created_by: str,
    note: str | None,
    id_factory: IdFactory,
    created_at: datetime,
) -> tuple[Assertion, bool]:
    """Insert one assertion, or find the identical one already active.

    Returns:
        The row that is active for the pair afterwards, and whether this call is
        what inserted it. The flag is what lets `er assert load` report "nothing to
        do" (S4.0 exit ``10``) without a second query per row.

    Raises:
        AssertionConflict: the pair already carries an active row of the other kind.
    """
    validated = _require_kind(kind)
    rec_a_key, rec_b_key = _canonical_pair(a, b)
    # The read and the write are one transaction: the precedence check is only
    # sound while nothing can insert between it and the row it permits.
    connection.execute("BEGIN TRANSACTION")
    try:
        existing = _active_for_pair(connection, rec_a_key, rec_b_key)
        if existing is not None:
            if existing.kind != validated:
                raise AssertionConflict(existing, validated)
            # An identical active assertion. S4.0 gives `er assert add` no exit `10`,
            # and a second row would break S5.0's filtered uniqueness, so the write
            # is skipped and the existing row is what the command reports.
            written, inserted = existing, False
        else:
            written = Assertion(
                assertion_id=id_factory.new(),
                rec_a_key=rec_a_key,
                rec_b_key=rec_b_key,
                kind=validated,
                active=True,
                created_by=created_by,
                created_at=created_at,
                note=note,
            )
            _insert(connection, written)
            inserted = True
    except BaseException:
        connection.execute("ROLLBACK")
        raise
    connection.execute("COMMIT")
    return written, inserted


def add_assertion(
    connection: duckdb.DuckDBPyConnection,
    *,
    a: str,
    b: str,
    kind: str,
    created_by: str,
    note: str | None = None,
    id_factory: IdFactory | None = None,
    created_at: datetime | None = None,
) -> Assertion:
    """Assert ``always`` or ``never`` over a pair of records (S4.4).

    ``a`` and ``b`` are unordered inputs, not columns: they are canonicalised
    through :func:`~er.entities.ids.canonicalize_pair`, so a caller passing them the
    wrong way round writes a canonical row rather than earning an error (S5.0, D9).

    Args:
        connection: a connection with the lake attached, per S4.0b.
        a, b: the two record keys, in either order.
        kind: ``always`` or ``never``.
        created_by: the steward, for `assertions.created_by`.
        note: free text, or ``None``.
        id_factory: the source of the minted `assertion_id`; production ULIDs by
            default, a counting factory in tests (S4.5.4, D10).
        created_at: the stamp for the row; now, in UTC, when omitted.

    Returns:
        The row that is active for the pair afterwards. Re-adding an assertion that
        is already active returns it unchanged and writes nothing.

    Raises:
        er.errors.ConfigError: a malformed key or an unknown kind (S4.0 exit ``2``).
        AssertionConflict: the pair already carries the other kind (S4.0 exit ``1``).
    """
    factory = id_factory if id_factory is not None else UlidFactory()
    written, _ = _add(
        connection,
        a=a,
        b=b,
        kind=kind,
        created_by=created_by,
        note=note,
        id_factory=factory,
        created_at=_stamp(created_at),
    )
    return written


def retract_assertion(
    connection: duckdb.DuckDBPyConnection,
    assertion_id: str,
    *,
    retracted_by: str,
    retracted_at: datetime | None = None,
) -> Assertion:
    """Retract one assertion: ``active=false``, stamped, never deleted (S4.4).

    The row survives because the assertion delta between two runs is computed from
    `retracted_at` (S4.5.1); a `DELETE` would make a retraction invisible to the
    next run's affected-node set rather than visible to it.

    Retracting an already-retracted row is a no-op that returns it unchanged: the
    stamps belong to the retraction that happened, and overwriting them would
    rewrite when the constraint stopped applying.

    Returns:
        The row as it now stands.

    Raises:
        er.errors.ConfigError: no row carries this `assertion_id` (S4.0 exit ``2``).
    """
    row = _by_id(connection, assertion_id)
    if row is None:
        raise ConfigError(
            f"{ASSERTIONS_RELATION} holds no row for assertion_id={assertion_id!r}; "
            f"assertions are referred to by canonical pair, so `er assert add` on the "
            f"pair is what mints one"
        )
    if not row.active:
        return row
    stamped = _stamp(retracted_at)
    connection.execute(_RETRACT_SQL, [retracted_by, stamped, assertion_id])
    return replace(row, active=False, retracted_by=retracted_by, retracted_at=stamped)


def active_assertions(connection: duckdb.DuckDBPyConnection) -> list[Assertion]:
    """Every ``active`` assertion, in canonical-pair order.

    THE reader of the assertion set: edge adjustment (S4.4), CONTRADICTION-1
    (S4.4.1) and the partition-level cut (S4.4.2) all start from this list, and the
    ordering is part of what makes reconciliation's total order well defined
    (S4.5.4).
    """
    rows = connection.execute(
        f"{_SELECT_SQL} WHERE active ORDER BY rec_a_key, rec_b_key, assertion_id"
    ).fetchall()
    return [_row_from(row) for row in rows]


def assertion_id_for(
    connection: duckdb.DuckDBPyConnection, rec_a_key: str, rec_b_key: str
) -> str | None:
    """The `assertion_id` active for a pair, or ``None``.

    The resolution step that lets everything else refer to an assertion by its
    canonical pair rather than by its minted ULID — which is what keeps a committed
    `assertions.csv` stable across runs, since S8.2.1's header carries no
    `assertion_id` column and this ticket may not add one.

    The arguments are canonicalised, so a caller may pass them in either order.

    Raises:
        er.errors.ConfigError: a malformed key (S4.0 exit ``2``).
    """
    pair = _canonical_pair(rec_a_key, rec_b_key)
    existing = _active_for_pair(connection, *pair)
    return None if existing is None else existing.assertion_id


def parse_assertions_csv(path: Path) -> list[AssertionInput]:
    """Read an S8.2.1 `assertions.csv`, header and phase vocabulary included.

    The header is compared BYTE FOR BYTE against :data:`ASSERTIONS_CSV_HEADER`. A
    file read positionally under a drifted header would land a `phase` value in
    `rec_a_key` and assert over records that do not exist, which is a corruption no
    later stage can detect — so it is refused here rather than tolerated.

    Raises:
        er.errors.ConfigError: the header is not byte-equal to S8.2.1's, a row has
            the wrong field count, `phase` is outside `{base, batch, refresh,
            resurrect}`, `kind` is outside `{always, never}`, or a record key is
            malformed. Every one is S4.0 exit ``2``.
    """
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    if not lines:
        raise ConfigError(
            f"{path}: file is empty; S8.2.1 requires the header {ASSERTIONS_CSV_HEADER!r}"
        )
    if lines[0] != ASSERTIONS_CSV_HEADER:
        raise ConfigError(
            f"{path}: header is {lines[0]!r}; S8.2.1's is literal and this ticket may "
            f"not add a column to it: {ASSERTIONS_CSV_HEADER!r}"
        )
    reader = csv.reader(io.StringIO(text))
    next(reader, None)
    rows: list[AssertionInput] = []
    for number, record in enumerate(reader, start=2):
        if not record:
            continue
        if len(record) != len(ASSERTIONS_CSV_COLUMNS):
            raise ConfigError(
                f"{path}:{number}: {len(record)} field(s), and S8.2.1's header declares "
                f"{len(ASSERTIONS_CSV_COLUMNS)}: {ASSERTIONS_CSV_HEADER!r}"
            )
        phase, rec_a_key, rec_b_key, kind, created_by, note = record
        if phase not in PHASES:
            raise ConfigError(
                f"{path}:{number}: phase={phase!r} is outside S8.2.1's vocabulary "
                f"{{{', '.join(PHASES)}}}"
            )
        if not created_by:
            raise ConfigError(f"{path}:{number}: created_by is empty")
        try:
            rows.append(
                AssertionInput(
                    phase=phase,
                    rec_a_key=_require_record_key(rec_a_key),
                    rec_b_key=_require_record_key(rec_b_key),
                    kind=_require_kind(kind),
                    created_by=created_by,
                    # `\N` is NULL; an empty field is the empty string, and S8.2.1
                    # makes the two distinct values rather than two spellings of one.
                    note=None if note == NULL_TOKEN else note,
                )
            )
        except InvalidRecordKeyError as exc:
            raise ConfigError(f"{path}:{number}: {exc}") from exc
    return rows


def load_assertions_csv(
    connection: duckdb.DuckDBPyConnection,
    path: Path,
    *,
    id_factory: IdFactory | None = None,
    created_at: datetime | None = None,
) -> list[Assertion]:
    """Apply every row of an S8.2.1 `assertions.csv` (S4.0 `er assert load`).

    EVERY row, not the rows of one phase: `assertions.csv` carries its `phase`
    column so that a scenario driver can slice the file before handing it over, and
    a loader that filtered here would make that column mean two different things.

    One `created_at` is stamped for the whole file, so a re-load is comparable
    against the first and no row's stamp depends on how long the file took to apply.

    Returns:
        The rows this call INSERTED, in file order. Empty when every row was
        already present and active, which is S4.0's exit ``10`` for the command.

    Raises:
        er.errors.ConfigError: the file violates S8.2.1 (exit ``2``).
        AssertionConflict: a row contradicts an active assertion of the other kind
            for the same pair (exit ``1``). The rows before it stay applied — S4.7
            gives the stage no cross-row transaction, and each row's own insert is
            atomic.
    """
    factory = id_factory if id_factory is not None else UlidFactory()
    stamped = _stamp(created_at)
    inserted: list[Assertion] = []
    for row in parse_assertions_csv(path):
        written, is_new = _add(
            connection,
            a=row.rec_a_key,
            b=row.rec_b_key,
            kind=row.kind,
            created_by=row.created_by,
            note=row.note,
            id_factory=factory,
            created_at=stamped,
        )
        if is_new:
            inserted.append(written)
    return inserted


def _find(parent: dict[str, str], key: str) -> str:
    """The representative of ``key``'s component, with the path compressed."""
    root = key
    while parent[root] != root:
        root = parent[root]
    while parent[key] != root:
        parent[key], key = root, parent[key]
    return root


def _union(parent: dict[str, str], a: str, b: str) -> None:
    """Merge the components of ``a`` and ``b``, keeping the lexically smaller root.

    The tie-break is not cosmetic: CONTRADICTION-1's finding names a component, and
    a representative chosen by insertion order would make the same assertion set
    produce a different diagnosis depending on scan order.
    """
    root_a, root_b = _find(parent, a), _find(parent, b)
    if root_a == root_b:
        return
    if root_b < root_a:
        root_a, root_b = root_b, root_a
    parent[root_b] = root_a


def check_contradiction_1(assertions: Iterable[Assertion]) -> list[Contradiction]:
    """CONTRADICTION-1: every ``never`` pair inside one always-closure component (S4.4.1).

    A PURE function over a sequence of assertions. It opens no connection, reads no
    relation and fails no run: the hard exit-`1` pre-clustering failure S4.4.1
    specifies is `er reconcile`'s, which owns the exit path and the no-snapshot
    guarantee (ER-074). What this returns is the diagnosis that failure reports.

    Only ``active`` rows count. A retracted `never` is not a constraint, so the same
    assertion set with the `never` retracted is clean — that difference is the whole
    reason retraction is a stamp rather than a delete.

    Args:
        assertions: any assertion set; typically :func:`active_assertions`'.

    Returns:
        One finding per offending ``never``, in canonical-pair order, each carrying
        the offending `assertion_id`s and the closure component. Empty when the set
        is satisfiable as a partition.
    """
    active = [assertion for assertion in assertions if assertion.active]
    always = [assertion for assertion in active if assertion.kind == ALWAYS]

    parent: dict[str, str] = {}
    for assertion in always:
        for key in assertion.pair:
            parent.setdefault(key, key)
    for assertion in always:
        _union(parent, *assertion.pair)

    components: dict[str, set[str]] = {}
    for key in parent:
        components.setdefault(_find(parent, key), set()).add(key)
    offenders: dict[str, list[str]] = {}
    for assertion in always:
        offenders.setdefault(_find(parent, assertion.rec_a_key), []).append(assertion.assertion_id)

    found: list[Contradiction] = []
    for assertion in active:
        if assertion.kind != NEVER:
            continue
        # A `never` whose endpoints are not both in the closure is satisfiable: the
        # partition can simply keep them apart. Only co-membership is unsatisfiable.
        if assertion.rec_a_key not in parent or assertion.rec_b_key not in parent:
            continue
        root = _find(parent, assertion.rec_a_key)
        if root != _find(parent, assertion.rec_b_key):
            continue
        found.append(
            Contradiction(
                rec_a_key=assertion.rec_a_key,
                rec_b_key=assertion.rec_b_key,
                never_assertion_id=assertion.assertion_id,
                always_assertion_ids=tuple(sorted(offenders[root])),
                component=frozenset(components[root]),
            )
        )
    return sorted(found, key=lambda finding: (finding.rec_a_key, finding.rec_b_key))
