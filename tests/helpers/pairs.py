"""The two candidate pair sets T-BLK-1 compares, derived once (S4.2, S5.0, S8.3).

S4.2 calls T-BLK-1 "the only thing converting 'mirrored' from aspiration into a checked
invariant": the DISTINCT canonicalised pair set derived from `int_blocking_keys` must
equal Splink's blocked pair set exactly. That comparison is only worth anything while
both sides are derived in one place — two copies of "the pair set" drift the same way
the two descriptions of the blocking logic drift, and the parity check would then be
comparing two implementations of the same mistake. So the derivations live here, and
the later oracles that need them (T-INC-3's parity pairs, the T-INV-1 finalizer, the
S8.5 metrics universe) import them rather than re-deriving.

Three properties of the derivation are normative and easy to lose:

* **`SELECT DISTINCT`, on the dbt side, is not an optimisation.** Splink deduplicates
  candidates across rules by excluding the pairs preceding rules already produced; a
  key-table self-join does not, so a pair carried by two `key_type`s arrives twice.
  Without the DISTINCT the two sets differ by multiplicity alone and T-BLK-1 fails for
  a reason that has nothing to do with the mirror (S4.2).
* **The NULL/empty policy holds on both sides.** A NULL or empty-string `key_value` is
  never emitted and never blocks. NULL is already excluded by the join's own equality —
  SQL never matches NULL to NULL — but the empty string is not, and it would block
  every record whose key expression concatenates a missing component, so the predicate
  is written out rather than left to the model that is supposed to have applied it.
* **Both sides come from one `blocking_rules_from_config(cfg)` call.** The dbt var
  payload and the `BlockingRuleCreator` list are the two elements of its return tuple,
  built from one `expr` string each. :func:`splink_blocked_pairs` therefore overrides
  whatever blocking rules the frozen model artifact carries with the ones the generator
  produced for `cfg`; a helper that blocked on the artifact's own rules would be
  checking the artifact against dbt rather than the config against both.

**Why the Splink side uses `deterministic_link()` and not `predict()`.** T-BLK-1 is a
claim about *blocking*, and `deterministic_link` stops there: it generates the pairs the
blocking rules produce and applies no threshold, so no pair can be filtered out of the
comparison by a score. `predict()` would reach the same pairs only when told to keep
every one of them, and a helper whose correctness depends on a threshold argument being
absent is a helper that quietly loses pairs the day someone adds one.

Splink computes term frequency for the `tf: true` columns on its way through that call.
That is not the D4/S4.3.3 violation it looks like: nothing here scores, nothing here
writes a `match_scores` row, and a term frequency cannot move a pair into or out of a
blocked set. The frozen `tf_lookup` rows are what a *scoring* path registers, and this
module is not one.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence, Set
from typing import Any, Final

import duckdb
from splink import Linker

from er.config.schema import Config
from er.entities.ids import canonicalize_pair
from er.lake.columns import STD_RECORD_COLUMNS
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.api import splink_api
from er.matching.model import UNIQUE_ID_COLUMN, blocking_rules_from_config
from er.matching.tf import STD_RECORDS_RELATION

__all__ = [
    "BLOCKING_KEYS_RELATION",
    "BLOCKING_RULES_KEY",
    "DBT_LABEL",
    "PARITY_CORPUS_RELATION",
    "SPLINK_LABEL",
    "Pair",
    "blocking_key_pair_rows",
    "canonical_pairs_from_blocking_keys",
    "pair_key_types",
    "splink_blocked_pairs",
    "symmetric_difference_report",
]

#: One canonical pair, `rec_a_key < rec_b_key` (S5.0). Both derivations return sets of
#: these, so a caller never has to ask which orientation a helper happened to emit.
Pair = tuple[str, str]

#: The relation the dbt side is derived from. It is NOT an input to scoring (S4.3.4):
#: Splink regenerates its own candidates, which is exactly what makes the two sets
#: independent enough for their equality to mean something.
BLOCKING_KEYS_RELATION: Final = "int_blocking_keys"

#: The bare local relation the corpus is copied into. Bare and local for the reason
#: `er.matching.full` copies its own: Splink resolves an input table's columns through
#: `information_schema.columns WHERE table_name = '<name>'`, which a lake-qualified name
#: matches no row of. A distinct name from the scorer's, so a suite that does both in
#: one connection cannot have one call replace the other's corpus.
PARITY_CORPUS_RELATION: Final = "er_blocking_parity_corpus"

#: The settings key holding the rules Splink blocks on, which
#: :func:`splink_blocked_pairs` replaces with the generator's own list.
BLOCKING_RULES_KEY: Final = "blocking_rules_to_generate_predictions"

#: How the two sides are named in a failure report. Constants because the report is
#: what a human reads first when parity breaks, and "a" and "b" tell them nothing.
DBT_LABEL: Final = "int_blocking_keys"
SPLINK_LABEL: Final = "splink"

_BLOCKING_KEYS: Final = f"{SCHEMA_QUALIFIER}.{BLOCKING_KEYS_RELATION}"
_STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.{STD_RECORDS_RELATION}"

_LEFT: Final = f"{UNIQUE_ID_COLUMN}_l"
_RIGHT: Final = f"{UNIQUE_ID_COLUMN}_r"

#: S4.2's NULL/empty policy, as the self-join's own predicate. `is not null` is
#: redundant against the equality above it and is stated anyway: this module is where
#: the policy is enforced for the pair set, and a reader should not have to know SQL's
#: NULL semantics to see that it is.
_USABLE_KEY: Final = "{side}.key_value is not null and {side}.key_value <> ''"

#: The self-join, once. `a.record_key < b.record_key` is the join's own self-pair guard
#: and its canonicalisation at the same time — which is why the Python side can treat
#: :func:`~er.entities.ids.canonicalize_pair` as a check rather than as a sort.
_JOIN: Final = f"""
      FROM {_BLOCKING_KEYS} AS a
      JOIN {_BLOCKING_KEYS} AS b
        ON a.key_type = b.key_type
       AND a.key_value = b.key_value
       AND a.record_key < b.record_key
     WHERE {_USABLE_KEY.format(side="a")}
       AND {_USABLE_KEY.format(side="b")}
"""

#: Every self-join row, WITH the multiplicity a pair carried by two `key_type`s has.
#: The un-DISTINCTed set exists so the DISTINCT above it is exercised rather than
#: assumed (S4.2).
_PAIR_ROWS_SQL: Final = f"""
    SELECT a.record_key AS rec_a_key, b.record_key AS rec_b_key, a.key_type
    {_JOIN}
     ORDER BY rec_a_key, rec_b_key, key_type
"""

#: The dbt-derived candidate set, as S4.2 requires it be computed.
_DISTINCT_PAIRS_SQL: Final = f"""
    SELECT DISTINCT a.record_key AS rec_a_key, b.record_key AS rec_b_key
    {_JOIN}
     ORDER BY rec_a_key, rec_b_key
"""

_CORPUS_SQL: Final = (
    f"CREATE OR REPLACE TABLE {PARITY_CORPUS_RELATION} AS "
    f"SELECT {', '.join(STD_RECORD_COLUMNS)} FROM {_STD_RECORDS}"
)


def blocking_key_pair_rows(connection: duckdb.DuckDBPyConnection) -> list[tuple[str, str, str]]:
    """Every `(rec_a_key, rec_b_key, key_type)` the key-table self-join produces.

    Not made distinct: a pair two `key_type`s both carry appears once per `key_type`,
    and a duplicated `(key_type, key_value, record_key)` row appears once per copy.
    That multiplicity is the thing :func:`canonical_pairs_from_blocking_keys` collapses,
    so it has to be observable somewhere for the collapse to be a checked property.

    Args:
        connection: a connection with the lake attached, whose `int_blocking_keys` has
            been built (S4.2).

    Returns:
        The rows in `(rec_a_key, rec_b_key, key_type)` order, every pair canonical.
    """
    rows = connection.execute(_PAIR_ROWS_SQL).fetchall()
    return [
        (*canonicalize_pair(str(rec_a_key), str(rec_b_key)), str(key_type))
        for rec_a_key, rec_b_key, key_type in rows
    ]


def canonical_pairs_from_blocking_keys(connection: duckdb.DuckDBPyConnection) -> set[Pair]:
    """The DISTINCT canonicalised pair set `int_blocking_keys` implies (S4.2, T-BLK-1).

    The DISTINCT is issued in SQL rather than obtained by collecting into a `set`,
    because S4.2 states the requirement about the query: "the dbt-derived pair set MUST
    be computed with `SELECT DISTINCT` over canonicalised `(rec_a_key, rec_b_key)`".
    Every row is then put back through :func:`~er.entities.ids.canonicalize_pair`, which
    is what keeps that helper the authority for an ordering the join had to express as a
    comparison — a pair that came back the other way round, or with its two keys equal,
    raises here rather than reaching a comparison as a phantom difference.

    Args:
        connection: a connection with the lake attached and `int_blocking_keys` built.

    Returns:
        The canonical pairs, each exactly once.
    """
    rows = connection.execute(_DISTINCT_PAIRS_SQL).fetchall()
    return {canonicalize_pair(str(rec_a_key), str(rec_b_key)) for rec_a_key, rec_b_key in rows}


def pair_key_types(connection: duckdb.DuckDBPyConnection) -> dict[Pair, tuple[str, ...]]:
    """Which `key_type`s would produce each pair, for the failure report.

    "Missing 3 pairs" is a count; "missing 3 pairs, all of them `email_exact`" names the
    rule that diverged, which is the first thing anyone reading a T-BLK-1 failure needs
    (S4.2: a parity failure is a defect in the generator or the macro, not in the test).

    Args:
        connection: a connection with the lake attached and `int_blocking_keys` built.

    Returns:
        Pair -> the `key_type`s carrying it, sorted and without repeats.
    """
    attribution: dict[Pair, set[str]] = {}
    for rec_a_key, rec_b_key, key_type in blocking_key_pair_rows(connection):
        attribution.setdefault((rec_a_key, rec_b_key), set()).add(key_type)
    return {pair: tuple(sorted(types)) for pair, types in attribution.items()}


def splink_blocked_pairs(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    settings: Mapping[str, Any],
) -> set[Pair]:
    """The pair set Splink blocks on, from the same generator dbt was built from (S4.2).

    The order below is the one S4.0b fixes and `er.matching.full` follows for the same
    reasons: the API first, because its constructor's ``SET schema`` is what decides
    where an unqualified ``CREATE OR REPLACE TABLE`` lands, and only then the corpus
    copy and the linker. Every intermediate Splink materializes therefore lives in the
    in-memory scratch schema and none of them reaches the lake (M17), which is what lets
    a caller assert :func:`~er.matching.api.assert_no_splink_relations_in_lake` after
    this returns.

    Args:
        connection: an open S4.0b connection — a ``:memory:`` primary database with the
            lake attached by alias — whose `int_std_records` has been built.
        cfg: the validated S6 document. Its `blocking:` block is the single source of
            truth, and the rules come from :func:`~er.matching.model.blocking_rules_from_config`,
            the same call whose other element the dbt var payload is.
        settings: the frozen model document (S4.3.2 item 6). Used verbatim except for
            its blocking rules, which are replaced: the artifact records the rules the
            model was *trained* under, and what T-BLK-1 compares is the rules `cfg`
            declares now.

    Returns:
        The canonical pairs Splink's blocking generates, each exactly once — Splink
        deduplicates across rules by excluding preceding rules' pairs, so the set is
        already distinct and the `SELECT DISTINCT` below only says so.

    Raises:
        AssertionError: the blocked relation carries no `record_key_l`/`record_key_r`
            columns, which would otherwise surface as an empty pair set — a T-BLK-1
            failure attributed to blocking rather than to the projection.
    """
    _, rules = blocking_rules_from_config(cfg)
    api = splink_api(connection)
    connection.execute(_CORPUS_SQL)
    blocking_settings: dict[str, Any] = {**settings, BLOCKING_RULES_KEY: list(rules)}
    linker = Linker(PARITY_CORPUS_RELATION, settings=blocking_settings, db_api=api)

    blocked = linker.inference.deterministic_link()
    relation = str(blocked.physical_name)
    columns = {
        str(row[0]) for row in connection.execute(f"DESCRIBE SELECT * FROM {relation}").fetchall()
    }
    missing = sorted({_LEFT, _RIGHT} - columns)
    assert not missing, (
        f"{relation} carries no {missing}; Splink names the blocked endpoints after "
        f"unique_id_column_name ({UNIQUE_ID_COLUMN!r}, S5.0) and the pair set cannot be "
        f"projected without them. Columns: {sorted(columns)}"
    )

    # The self-pair guard is stated even though `dedupe_only` blocking already emits
    # `l < r`: `canonicalize_pair` raises on two equal keys, and a Splink release that
    # started emitting one should fail as a parity difference, not as a helper crash.
    rows = connection.execute(
        f"SELECT DISTINCT {_LEFT}, {_RIGHT} FROM {relation} WHERE {_LEFT} <> {_RIGHT}"
    ).fetchall()
    return {canonicalize_pair(str(rec_a_key), str(rec_b_key)) for rec_a_key, rec_b_key in rows}


def _pair_lines(pairs: Set[Pair], key_types: Mapping[Pair, Sequence[str]] | None) -> list[str]:
    """One indented line per pair, carrying its `key_type` attribution when there is one."""
    lines: list[str] = []
    for pair in sorted(pairs):
        carried = () if key_types is None else tuple(key_types.get(pair, ()))
        attribution = f"  key_type={','.join(carried)}" if carried else "  key_type=<none>"
        lines.append(f"    {pair[0]} | {pair[1]}{attribution}")
    return lines


def symmetric_difference_report(
    a: Set[Pair],
    b: Set[Pair],
    *,
    key_types: Mapping[Pair, Sequence[str]] | None = None,
    a_label: str = DBT_LABEL,
    b_label: str = SPLINK_LABEL,
) -> str:
    """The T-BLK-1 failure message: which pairs differ, and which rule carried them.

    S8.3 requires the symmetric difference itself, "with the symmetric difference
    printed on failure" — a count would say that the mirror broke without saying where,
    and the whole value of T-BLK-1 is that it localises the drift to a `key_type` before
    anyone goes looking for it in clustering.

    Args:
        a: the first pair set, conventionally the dbt-derived one.
        b: the second, conventionally Splink's.
        key_types: pair -> the `key_type`s that would produce it, from
            :func:`pair_key_types`. Pairs with no entry are reported as `<none>`, which
            is itself informative: a pair Splink blocks that no `key_type` carries is
            the divergence, stated.
        a_label: how the first set is named in the report.
        b_label: how the second is named.

    Returns:
        A multi-line report. Sets that agree get one line saying so, so the string is
        safe to build unconditionally as an assertion message.
    """
    only_a = set(a) - set(b)
    only_b = set(b) - set(a)
    header = (
        f"blocking parity (T-BLK-1, S4.2): {a_label} has {len(a)} pair(s), {b_label} has {len(b)}"
    )
    if not only_a and not only_b:
        return f"{header}; the two sets are equal"
    return "\n".join(
        [
            header,
            f"  only in {a_label} ({len(only_a)}):",
            *_pair_lines(only_a, key_types),
            f"  only in {b_label} ({len(only_b)}):",
            *_pair_lines(only_b, key_types),
        ]
    )
