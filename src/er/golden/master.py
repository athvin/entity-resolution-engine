"""Master-record election policies (docs/backend-design.md §10).

Survivorship elects field *values*; an external merge needs a master *record* —
the one member of an entity that survives when the others are folded into it.
This module is the single owner of that election so the merge-plan export, the
future apply service, and steward entity-edit targeting all elect the same
master for the same entity.

Three properties are normative here and stated nowhere else:

* The vocabulary is closed: :data:`MASTER_ELECTION_POLICIES`. An unknown policy
  is a :class:`MasterElectionError`, never a silent fallback, matching the
  scorer-registry discipline of :mod:`er.embeddings.coherence`.
* Every policy is a total order. Each ``ORDER BY`` ends with the mandatory
  ``record_key`` tiebreak — the survivorship doctrine (S6.1): a policy that can
  tie is a policy whose winner depends on scan order.
* ``most_attributes`` is the default and reproduces the election the merge-plan
  export shipped with byte-for-byte: the member contributing the most golden
  attributes per ``golden_lineage``, ties to the lowest ``record_key``. Members
  with no lineage row win nothing and are never elected under it.

The module renders SQL text rather than executing it: the read path owns its
connections, snapshot pinning and schema qualification, and this module owns
only the semantics. The rendered fragment references an ``affected(entity_id)``
CTE the caller must define — the set of entities to elect masters for.
"""

from __future__ import annotations

from collections.abc import Mapping
from typing import Final

from er.lake.columns import GOLDEN_SURVIVABLE_COLUMNS

__all__ = [
    "DEFAULT_MASTER_ELECTION_POLICY",
    "MASTER_ELECTION_POLICIES",
    "MasterElectionError",
    "masters_cte_sql",
]

#: The closed policy vocabulary, in documentation order.
MASTER_ELECTION_POLICIES: Final[tuple[str, ...]] = (
    "most_attributes",
    "source_priority",
    "most_recent",
    "oldest",
    "most_complete",
)

DEFAULT_MASTER_ELECTION_POLICY: Final[str] = "most_attributes"

# The sentinel a source missing from `source_ranks` sorts under: after every
# ranked source, before nothing. A missing rank is a config/lake skew (a source
# was removed from the document while its records remain), and demoting it is
# the deterministic choice that never elects a record the config no longer
# describes while ranked members exist.
_UNRANKED_SENTINEL: Final[int] = 2**31 - 1


class MasterElectionError(ValueError):
    """An election the vocabulary does not describe; the caller maps it to 422/exit 2."""


def _quote_literal(value: str) -> str:
    """A single-quoted SQL string literal with embedded quotes doubled."""
    escaped = value.replace("'", "''")
    return f"'{escaped}'"


def _rank_case(source_ranks: Mapping[str, int]) -> str:
    """The ``CASE`` expression ordering members by their source's priority rank."""
    arms = " ".join(
        f"WHEN {_quote_literal(source)} THEN {int(rank)}"
        for source, rank in sorted(source_ranks.items())
    )
    return f"COALESCE(CASE m.source_system {arms} END, {_UNRANKED_SENTINEL})"


def _completeness_expr() -> str:
    """How many survivable attributes a member carries, over the S5 column set."""
    terms = " + ".join(
        f"(CASE WHEN s.{column} IS NOT NULL THEN 1 ELSE 0 END)"
        for column in GOLDEN_SURVIVABLE_COLUMNS
    )
    return f"({terms})"


def masters_cte_sql(
    policy: str = DEFAULT_MASTER_ELECTION_POLICY,
    *,
    schema_qualifier: str,
    source_ranks: Mapping[str, int] | None = None,
) -> str:
    """The body of a ``masters(entity_id, master_key)`` CTE for one policy.

    Args:
        policy: a member of :data:`MASTER_ELECTION_POLICIES`.
        schema_qualifier: the qualifier the caller reads lake relations under,
            e.g. ``lake.main``; interpolated verbatim, so it MUST come from the
            caller's own configuration, never from user input.
        source_ranks: ``source_system -> priority_rank``, required by (and only
            by) the ``source_priority`` policy; the caller reads it from the
            tenant's validated config document.

    Raises:
        MasterElectionError: the policy is unknown, or ``source_priority`` was
            asked for without ``source_ranks``.
    """
    if policy not in MASTER_ELECTION_POLICIES:
        raise MasterElectionError(
            f"{policy!r} is not a master-election policy; the vocabulary is "
            f"{list(MASTER_ELECTION_POLICIES)}"
        )

    if policy == "most_attributes":
        # The shipped election, byte-compatible: lineage contribution counts.
        return f"""
          SELECT entity_id, record_key AS master_key
          FROM (
            SELECT entity_id, record_key,
                   row_number() OVER (
                     PARTITION BY entity_id
                     ORDER BY won_attributes DESC, record_key
                   ) AS rank
            FROM (
              SELECT l.entity_id, l.record_key, count(*) AS won_attributes
              FROM {schema_qualifier}.golden_lineage l
              JOIN affected a ON a.entity_id = l.entity_id
              GROUP BY l.entity_id, l.record_key
            )
          ) WHERE rank = 1
        """

    if policy == "source_priority":
        if source_ranks is None:
            raise MasterElectionError(
                "the source_priority policy needs the tenant's sources.<name>.priority_rank mapping"
            )
        order = f"{_rank_case(source_ranks)} ASC"
    elif policy == "most_recent":
        # `ingested_at` is NOT NULL (S5), so the COALESCE never yields NULL.
        order = "COALESCE(s.updated_at_source, s.ingested_at) DESC"
    elif policy == "oldest":
        order = "COALESCE(s.updated_at_source, s.ingested_at) ASC"
    else:  # most_complete
        order = f"{_completeness_expr()} DESC"

    return f"""
      SELECT entity_id, master_key
      FROM (
        SELECT m.entity_id, m.record_key AS master_key,
               row_number() OVER (
                 PARTITION BY m.entity_id
                 ORDER BY {order}, m.record_key
               ) AS rank
        FROM {schema_qualifier}.entity_membership m
        JOIN affected a ON a.entity_id = m.entity_id
        JOIN {schema_qualifier}.int_std_records s ON s.record_key = m.record_key
      ) WHERE rank = 1
    """
