"""Which `match_scores` row is *the* edge for a canonical pair (S4.5.1, S4.3.4).

`match_scores` is cumulative: it is never truncated per run, rows written by different
runs sit side by side, and invalidation is an in-place `UPDATE` rather than a delete
(S4.3.4). So "the current edge set" is not "this run's scored pairs" — S4.5.1 is
explicit that an implementer who reads it that way "will spuriously fragment every
touched entity". It is the set of currently-active rows at the run's
`(model_version, tf_snapshot_id)`, one per canonical pair.

That is a reading, and a reading is exactly the kind of thing three call sites invent
three times. Clustering (S4.5.2), reconciliation (S4.5.4) and the T-INV-1 invariant
finalizer all need it, and if any one of them spelled `is_active` or the key filter
differently the partition it computed would disagree with the partition the others
checked — silently, because every arm would still return well-formed edges. This module
is therefore the single definition, in three forms of one query:

* :func:`current_edges_sql` — the composable SQL, for a caller that wants to join it
  into a larger statement rather than pull it into Python.
* :func:`materialize_current_edges` — the same query as a named relation, created in
  the **in-memory** database. S4.0b confines the S4.5 loop's intermediates there: only
  the final labelling is written to the lake, so a helper that created a relation in
  `lake` would commit a snapshot per iteration.
* :func:`current_edges` — the eager `(rec_a_key, rec_b_key, match_probability)` list.

**Why a total order is needed at all.** `match_scores`' logical key already includes
`model_version` and `tf_snapshot_id` (S5.0), so within one key there should be exactly
one row. DuckLake enforces no `UNIQUE` constraint — it enforces `NOT NULL` and nothing
else (S5.0) — so "should" is a claim about the writer, not about the engine. Rather
than pick a row arbitrarily when the claim is false, this module raises
:class:`DuplicateEdgeKeyError` by default and resolves by :data:`CURRENT_EDGE_ORDER`
only when a caller explicitly asks for `strict=False`. A duplicate key is a broken
`MERGE INTO`, and a clustering run that quietly averaged over it would turn a writer
bug into a wrong partition nobody could trace back.

Scope, deliberately: what this module does NOT do is the rest of S4.5.1's affected-edge
query — the `cut_edges` exclusion, the assertion adjustment ("minus active `never`
pairs, plus active `always` pairs at `p = 1.0`", S4.4) and the restriction to the
affected nodes. Those compose *on top of* this selection and belong to the affected-edge
set that builds it. Nothing here writes, invalidates or deletes a row.
"""

from __future__ import annotations

import re
from math import isfinite
from typing import Final

import duckdb

from er.entities.ids import canonicalize_pair
from er.errors import PreconditionFailure
from er.lake.ducklake import sql_literal
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER

__all__ = [
    "CURRENT_EDGES_RELATION",
    "CURRENT_EDGE_ORDER",
    "EDGE_COLUMNS",
    "MATCH_SCORES_RELATION",
    "DuplicateEdgeKeyError",
    "current_edges",
    "current_edges_sql",
    "materialize_current_edges",
]

#: The cumulative relation every form below reads, and nothing else. Always
#: `lake.main.`-qualified: S4.0b forbids DuckLake being the default catalog, so an
#: unqualified name here would resolve against the in-memory database this module
#: materializes into and silently read an empty relation.
MATCH_SCORES_RELATION: Final = "match_scores"

_MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.{MATCH_SCORES_RELATION}"

#: The default name :func:`materialize_current_edges` creates. `er_`-prefixed so it
#: cannot collide with a Splink intermediate in the same in-memory database.
CURRENT_EDGES_RELATION: Final = "er_current_edges"

#: The three columns S4.5.1's normative query projects, in its order. A consumer joins
#: on the pair and thresholds on the probability; nothing downstream of clustering needs
#: the provenance columns, and projecting them would invite a caller to read `run_id`
#: and re-derive "this run's pairs" — the exact misreading S4.5.1 warns against.
EDGE_COLUMNS: Final[tuple[str, ...]] = ("rec_a_key", "rec_b_key", "match_probability")

#: The pair half and the key half of S5.0's logical key for `match_scores`, split
#: because this module treats them differently: the key half is *bound* by the caller's
#: arguments, and the pair half is what the result holds one row per.
PAIR_COLUMNS: Final[tuple[str, ...]] = ("rec_a_key", "rec_b_key")
SCOPE_COLUMNS: Final[tuple[str, ...]] = ("model_version", "tf_snapshot_id")

_LOGICAL_KEY: Final[tuple[str, ...]] = REGISTRY[MATCH_SCORES_RELATION].keys[0].columns

if set(PAIR_COLUMNS) | set(SCOPE_COLUMNS) != set(_LOGICAL_KEY):
    # At import, not at query time. The duplicate-key check below groups by
    # `PAIR_COLUMNS` *within* a bound `SCOPE_COLUMNS`, which is the logical key only
    # while these two lists partition it. A key that grew a column in S5 would
    # otherwise leave this module reporting duplicates that are not duplicates.
    raise AssertionError(
        f"{MATCH_SCORES_RELATION}'s logical key is {_LOGICAL_KEY}; this module splits it "
        f"into {PAIR_COLUMNS} + {SCOPE_COLUMNS} (S5.0)"
    )

#: **The total order**, and the one place it is written down.
#:
#: It is a tie-break, not a filter: within one `(rec_a_key, rec_b_key, model_version,
#: tf_snapshot_id)` there should be exactly one row (S4.3.4), so reaching for this order
#: at all means the logical key has been violated and `strict=False` asked for a row
#: anyway. Most-recently-scored wins, and `run_id` breaks a tie between two rows scored
#: at the same instant — ULIDs are lexically ordered by mint time (D10), so the later
#: run wins there too.
#:
#: It is total over every pair of rows a *writer* could plausibly leave behind, and not
#: over every pair of rows imaginable: two rows for one key carrying the same
#: `scored_at` and the same `run_id` are one run writing one key twice in one statement,
#: which no `MERGE INTO` produces. `strict=True` — the default — refuses both cases
#: before the order is consulted.
CURRENT_EDGE_ORDER: Final[tuple[str, ...]] = ("scored_at DESC", "run_id DESC")

#: The window's rank column. `er_`-prefixed so it cannot shadow an S5 column name if
#: `match_scores` ever grows one, since the outer select filters on it by name.
_RANK_COLUMN: Final = "er_edge_rank"

#: A bare, unquoted SQL identifier. :func:`materialize_current_edges` interpolates the
#: relation name it is given into a `CREATE` statement, so it is the one caller-supplied
#: string in this module that does not reach SQL as a literal.
_IDENTIFIER_RE: Final = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


class DuplicateEdgeKeyError(PreconditionFailure):
    """More than one `match_scores` row shares one logical key (S5.0, S4.3.4).

    A precondition failure (S4.0 exit ``3``) rather than a stage failure: the stage has
    not begun, and the fault is upstream in whatever wrote the second row. S4.3.4 is
    unambiguous that the `MERGE INTO` leaves "at most one row per key regardless of
    `is_active`", so a second row is a broken writer and re-running the reader will not
    fix it. The operator action is to repair `match_scores`, not to retry.
    """


def _validate_key(model_version: str, tf_snapshot_id: str) -> None:
    """Refuse an empty key component.

    Both are `NOT NULL` on every `match_scores` row (S5), so an empty string matches
    nothing — which would make an empty edge set look like a corpus with no matches
    rather than like the caller error it is.
    """
    for name, value in (("model_version", model_version), ("tf_snapshot_id", tf_snapshot_id)):
        if not value:
            raise ValueError(
                f"{name} must not be empty; it selects the match_scores rows this run "
                f"clusters (S4.5.1), and no row carries an empty one (S5)"
            )


def _probability_literal(min_probability: float) -> str:
    """`min_probability` as a SQL numeric literal.

    Raises:
        ValueError: the value is not finite, or lies outside ``[0, 1]``. A `NaN`
            threshold makes every `>=` comparison false and would empty the edge set
            without failing anything; an infinite one does the same in one direction.
    """
    if not isfinite(min_probability):
        raise ValueError(
            f"min_probability must be a finite probability, got {min_probability!r}; "
            f"a non-finite threshold empties the edge set instead of failing"
        )
    if not 0.0 <= min_probability <= 1.0:
        raise ValueError(
            f"min_probability is a probability and must lie in [0, 1], got {min_probability!r} "
            f"(S4.3: all config thresholds are probabilities)"
        )
    return repr(float(min_probability))


def current_edges_sql(
    model_version: str,
    tf_snapshot_id: str,
    *,
    min_probability: float | None = None,
    include_inactive: bool = False,
) -> str:
    """The SQL selecting exactly one row per canonical pair at one key (S4.5.1).

    The returned statement projects :data:`EDGE_COLUMNS` and is self-contained: the key
    values are rendered as SQL literals rather than left as parameter markers, because a
    composable fragment carries no parameter list for the caller to bind and a caller
    that spliced it into a larger statement would have to reconstruct the ordinal
    positions of markers it never wrote.

    One row per pair is enforced with `row_number()` over
    :data:`CURRENT_EDGE_ORDER` rather than assumed. The assumption would be correct on a
    well-formed table (S4.3.4), and the whole reason this module exists is that DuckLake
    enforces nothing (S5.0) — a `SELECT` that trusted the key would return two rows for a
    violated one and hand clustering a duplicated edge.

    Args:
        model_version: the run's model version; rows at any other are a different
            scoring problem (INV-SCORE, S4.3.3) and are excluded.
        tf_snapshot_id: the run's TF snapshot, for the same reason.
        min_probability: when given, keep only ``match_probability >= min_probability``.
            **Inclusive**, matching S4.5.1's ``p >= :auto_merge``: a pair at exactly
            `auto_merge` is a match (S4.3) and is clustered.
        include_inactive: when true, keep rows invalidated by S4.5.5 as well. Off by
            default, which is the S4.5.1 reading; a caller turns it on to inspect what
            was invalidated, never to cluster it.

    Returns:
        A `SELECT` statement, usable standalone or as a subquery.

    Raises:
        ValueError: a key component is empty, or `min_probability` is not a finite
            probability.
    """
    _validate_key(model_version, tf_snapshot_id)

    predicates = [
        f"model_version = {sql_literal(model_version)}",
        f"tf_snapshot_id = {sql_literal(tf_snapshot_id)}",
    ]
    if not include_inactive:
        # Spelled as the bare boolean column, as S4.5.1 spells it: `is_active` is
        # `NOT NULL` (S5), so there is no three-valued case to guard.
        predicates.append("is_active")
    if min_probability is not None:
        predicates.append(f"match_probability >= {_probability_literal(min_probability)}")

    projection = ", ".join(EDGE_COLUMNS)
    partition = ", ".join(PAIR_COLUMNS)
    order = ", ".join(CURRENT_EDGE_ORDER)
    where = "\n           AND ".join(predicates)
    return f"""
SELECT {projection}
  FROM (SELECT {projection},
               row_number() OVER (PARTITION BY {partition}
                                      ORDER BY {order}) AS {_RANK_COLUMN}
          FROM {_MATCH_SCORES}
         WHERE {where}) AS ranked
 WHERE {_RANK_COLUMN} = 1
"""


_DUPLICATE_KEY_SQL: Final = f"""
SELECT {", ".join(PAIR_COLUMNS)}, count(*) AS row_count
  FROM {_MATCH_SCORES}
 WHERE {" AND ".join(f"{column} = ?" for column in SCOPE_COLUMNS)}
 GROUP BY {", ".join(PAIR_COLUMNS)}
HAVING count(*) > 1
 ORDER BY {", ".join(PAIR_COLUMNS)}
"""


def _assert_one_row_per_key(
    connection: duckdb.DuckDBPyConnection, model_version: str, tf_snapshot_id: str
) -> None:
    """Refuse a `(model_version, tf_snapshot_id)` whose logical key is violated.

    Deliberately **unfiltered** by `is_active` and by `min_probability`. S5.0 states
    `match_scores`' key unfiltered, and S4.3.4 makes the point explicitly — "at most one
    row per key regardless of `is_active`". A key holding one active and one invalidated
    row passes a filtered check and is still a broken `MERGE INTO`, and it is the shape a
    botched S4.5.5 invalidation actually produces.

    Raises:
        DuplicateEdgeKeyError: at least one pair has more than one row. The message names
            the first pair in canonical order and counts the rest, so the operator has a
            row to go and look at rather than a total.
    """
    violations = connection.execute(_DUPLICATE_KEY_SQL, [model_version, tf_snapshot_id]).fetchall()
    if not violations:
        return
    rec_a_key, rec_b_key, row_count = violations[0]
    also = "" if len(violations) == 1 else f", and {len(violations) - 1} further pair(s) like it"
    raise DuplicateEdgeKeyError(
        f"{MATCH_SCORES_RELATION} holds {row_count} rows for the pair "
        f"({rec_a_key!r}, {rec_b_key!r}) at model_version={model_version!r}, "
        f"tf_snapshot_id={tf_snapshot_id!r}{also}. S5.0's logical key "
        f"{tuple(_LOGICAL_KEY)} permits one row per key regardless of is_active "
        f"(S4.3.4), so this is a MERGE INTO that did not merge. Repair the relation; "
        f"pass strict=False to resolve by {CURRENT_EDGE_ORDER} instead."
    )


def _canonical(rec_a_key: str, rec_b_key: str) -> tuple[str, str]:
    """One returned pair, checked against S5.0's canonical ordering.

    Canonicalisation happens in one shared helper at *write* time and "readers never
    perform a two-sided join" (S5.0) — so a reader that found `(b, a)` on a row would
    have joined the wrong way round everywhere else too, silently. Checking here rather
    than swapping keeps that a failure instead of a repair: a non-canonical row means
    some writer bypassed `canonicalize_pair`, and the rows it wrote are not addressable
    by the pair key anything downstream uses.

    Raises:
        PreconditionFailure: the stored pair is a self-pair or is not in canonical order.
    """
    try:
        canonical = canonicalize_pair(rec_a_key, rec_b_key)
    except ValueError as invalid:
        raise PreconditionFailure(
            f"{MATCH_SCORES_RELATION} holds a self-pair ({rec_a_key!r}, {rec_b_key!r}); "
            f"S4.3.4 drops self-pairs before persisting and S5.0 requires "
            f"rec_a_key < rec_b_key"
        ) from invalid
    if canonical != (rec_a_key, rec_b_key):
        raise PreconditionFailure(
            f"{MATCH_SCORES_RELATION} holds ({rec_a_key!r}, {rec_b_key!r}), which is not "
            f"canonical: S5.0 requires rec_a_key < rec_b_key on every pair row, written "
            f"through er.entities.ids.canonicalize_pair"
        )
    return canonical


def current_edges(
    connection: duckdb.DuckDBPyConnection,
    model_version: str,
    tf_snapshot_id: str,
    *,
    min_probability: float | None = None,
    include_inactive: bool = False,
    strict: bool = True,
) -> list[tuple[str, str, float]]:
    """The current edge set as `(rec_a_key, rec_b_key, match_probability)` triples.

    The eager form, for the Python-side consumers: the label-propagation seed, the
    reconciler's overlap matrix and the T-INV-1 finalizer all want the edges in hand
    rather than as a relation to join.

    Returned in canonical pair order, which makes two calls over an unchanged relation
    equal as *sequences* and not merely as multisets. That is stronger than the S4.5.1
    query needs and costs nothing: the pair is unique in the result, so sorting on it is
    a total order and removes the only source of run-to-run variation a set-returning
    query has.

    Args:
        connection: a connection with the lake attached (S4.0b). Nothing is written to
            it — this function reads, and creates no relation at all.
        model_version: the run's model version.
        tf_snapshot_id: the run's TF snapshot.
        min_probability: inclusive lower bound on `match_probability`; `auto_merge` for
            the clustering edge set (S4.5.1), omitted to see everything scored.
        include_inactive: include rows invalidated by S4.5.5.
        strict: refuse a violated logical key rather than resolving it. Default on; see
            :class:`DuplicateEdgeKeyError` for why the refusal is the safer default.

    Returns:
        One triple per canonical pair, in `(rec_a_key, rec_b_key)` order.

    Raises:
        DuplicateEdgeKeyError: `strict` and a pair has more than one row.
        er.errors.PreconditionFailure: a returned row is a self-pair or is not in
            canonical order (S5.0).
        ValueError: a key component is empty, or `min_probability` is not a finite
            probability.
    """
    statement = current_edges_sql(
        model_version,
        tf_snapshot_id,
        min_probability=min_probability,
        include_inactive=include_inactive,
    )
    if strict:
        _assert_one_row_per_key(connection, model_version, tf_snapshot_id)
    rows = connection.execute(statement).fetchall()
    edges = [
        (*_canonical(str(rec_a_key), str(rec_b_key)), float(match_probability))
        for rec_a_key, rec_b_key, match_probability in rows
    ]
    return sorted(edges, key=lambda edge: (edge[0], edge[1]))


def materialize_current_edges(
    connection: duckdb.DuckDBPyConnection,
    model_version: str,
    tf_snapshot_id: str,
    *,
    min_probability: float | None = None,
    include_inactive: bool = False,
    strict: bool = True,
    name: str = CURRENT_EDGES_RELATION,
) -> str:
    """Create the current edge set as a named relation and return its name.

    A **temporary view**, and both halves of that matter. Temporary, because S4.0b
    confines the S4.5 loop's intermediates to the in-memory database — only the final
    labelling is written to the lake, and a relation created in `lake` would commit a
    DuckLake snapshot for a set of edges that is a working value, not a result. A view
    rather than a table, because the caller composes it into a larger statement and a
    view costs nothing to create and cannot go stale against the rows it names.

    Args:
        connection: a connection with the lake attached (S4.0b). The view is created in
            the in-memory database; `lake` is only read.
        model_version: the run's model version.
        tf_snapshot_id: the run's TF snapshot.
        min_probability: inclusive lower bound on `match_probability`.
        include_inactive: include rows invalidated by S4.5.5.
        strict: refuse a violated logical key, checked once here rather than on every
            query of the view — a view cannot raise.
        name: the relation name to create. Replaced if it exists, so one caller may
            re-materialize under a changing threshold without dropping first.

    Returns:
        `name`, so a caller can write ``f"... FROM {materialize_current_edges(...)}"``
        without restating it.

    Raises:
        DuplicateEdgeKeyError: `strict` and a pair has more than one row.
        ValueError: `name` is not a bare SQL identifier, a key component is empty, or
            `min_probability` is not a finite probability.
    """
    if _IDENTIFIER_RE.fullmatch(name) is None:
        raise ValueError(
            f"name must be a bare SQL identifier, got {name!r}; it is interpolated into "
            f"a CREATE statement rather than bound"
        )
    statement = current_edges_sql(
        model_version,
        tf_snapshot_id,
        min_probability=min_probability,
        include_inactive=include_inactive,
    )
    if strict:
        _assert_one_row_per_key(connection, model_version, tf_snapshot_id)
    connection.execute(f"CREATE OR REPLACE TEMP VIEW {name} AS {statement}")
    return name
