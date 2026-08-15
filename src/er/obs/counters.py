"""The S5.2 counter vocabulary: eleven typed columns, everything else in JSON.

S5.2 splits counters in two and calls the split normative:

1. **Promoted counters** — :data:`PROMOTED_COUNTERS`, the closed set that is a typed
   `run_stages` column. Promoting a name to a column is a schema change under S5.1,
   so this module never grows the set; it re-exports the registry's tuple, which is
   the same object `run_stages` builds its counter columns from
   (:mod:`er.lake.model`). Two lists would be one list and one stale copy.
2. **Per-stage counters** — the free-form names each S4 subsection ends with. They
   go into the `run_stages.counters` JSON object. A stage MAY invent a name there
   and MUST NOT invent a column, which is why :meth:`StageCounters.promote` raises
   on an unlisted name rather than quietly widening the schema.

**Completeness (S5.2).** A stage's payload is the *union* of the names its S4
subsection declares and every promoted counter it actually wrote. The S4 lists
deliberately omit `rows_in`/`rows_out`, so neither list restates the other and the
JSON object alone is still a complete record of the stage. :meth:`StageCounters.payload`
is that union, and it is the only thing that writes the column.

A declared name a stage never measured is emitted as ``null`` rather than dropped:
"the stage does not report this" and "the stage reports zero" are different facts,
and a reader that cannot tell them apart cannot use the payload as a record.
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Mapping
from types import MappingProxyType
from typing import Final

from er.lake.model import PROMOTED_COUNTERS

__all__ = [
    "DECLARED_COUNTERS",
    "PROMOTED_COUNTERS",
    "StageCounters",
    "UnknownCounterError",
]

#: The per-stage names each S4 subsection ends with, keyed by `run_stages.stage`.
#: Transcribed from those "**Counters**" paragraphs and nowhere else: this is the
#: (b)-side of the S5.2 completeness rule, so a stage implementation supplies
#: *values* for these names and never a different set of names. Stages S4 gives no
#: counter list (`init`, `train`, `maintain`, `reset`) are absent rather than mapped
#: to an empty tuple, so "S4 declares none" stays distinguishable from "unlisted".
DECLARED_COUNTERS: Final[Mapping[str, tuple[str, ...]]] = MappingProxyType(
    {
        "ingest": (
            "files",
            "new_count",
            "changed_count",
            "unchanged_count",
            "tombstone_count",
            "resurrected_count",
            "ingest_batch_id",
            "duration_ms",
        ),
        "standardize": (
            "models_run",
            "stg_rows_appended",
            "std_rows_current",
            "blocking_rows",
            "blocking_keys_by_type",
            "tombstones_excluded",
            "duration_ms",
        ),
        "match": (
            "mode",
            "model_version",
            "tf_snapshot_id",
            "candidate_pairs",
            "pairs_scored",
            "pairs_above_auto_merge",
            "pairs_in_gray_band",
            "review_queue_added",
            "review_queue_refreshed",
            "duration_ms",
        ),
        "reconcile": (
            "affected_entities",
            "affected_edges",
            "label_prop_iterations",
            "clusters_out",
            "entities_created",
            "entities_merged",
            "entities_split",
            "entities_retired",
            "members_added",
            "members_removed",
            "edges_cut",
            "review_queue_added",
            "events_emitted",
            "duration_ms",
        ),
        "assemble": (
            "entities_touched",
            "entities_rebuilt",
            "entities_reaped",
            "lineage_rows",
            "tiebreak_deterministic_count",
            "duration_ms",
        ),
    }
)


class UnknownCounterError(ValueError):
    """A name was written as a typed column and S5.2 does not promote it.

    Raised rather than accepted, because the alternative failure is silent: the
    column does not exist, the write is dropped or the DDL drifts, and the stage
    reports a counter no reader can find. The remedy is the JSON payload, which is
    exactly what the message names.
    """

    def __init__(self, name: str) -> None:
        super().__init__(
            f"{name!r} is not a promoted counter; write it to the counters JSON "
            f"payload instead. Promoting a counter to a column is a schema change "
            f"under S5.1. Promoted: {', '.join(PROMOTED_COUNTERS)}"
        )
        self.name = name


class StageCounters:
    """What one stage counted: eleven possible columns plus a JSON payload.

    Args:
        declared: the per-stage names this stage's S4 subsection lists, usually
            ``DECLARED_COUNTERS[stage]``. They appear in the payload whether or not
            the stage measured them, which is what makes an omission visible.
    """

    def __init__(self, declared: Iterable[str] = ()) -> None:
        self.declared: tuple[str, ...] = tuple(declared)
        #: Promoted names written, in write order; the source of the typed columns.
        self._promoted: dict[str, int | None] = {}
        #: Free-form names, whose values need only be JSON-serialisable — S4.3's
        #: list carries `mode`, `model_version` and `tf_snapshot_id`, which are
        #: strings, and S4.2's `blocking_keys_by_type`, which is an object.
        self._extra: dict[str, object] = {}

    def promote(self, name: str, value: int | None) -> None:
        """Write ``name`` to its typed `run_stages` column.

        Raises:
            UnknownCounterError: ``name`` is not one of the eleven (S5.2).
        """
        if name not in PROMOTED_COUNTERS:
            raise UnknownCounterError(name)
        self._promoted[name] = None if value is None else int(value)

    def set(self, name: str, value: object) -> None:
        """Record ``name``, routing it to the column S5.2 promotes it to, if any.

        THE entry point a stage uses. Routing here rather than at each call site is
        what keeps the completeness rule true by construction: a stage that records
        `candidate_pairs` gets both the typed column and the payload entry, and one
        that records `pairs_in_gray_band` gets the payload entry alone, without
        either caller having to know which list the name is on.
        """
        if name in PROMOTED_COUNTERS:
            if value is not None and not isinstance(value, int):
                # Every promoted counter is a `BIGINT` column (S5.2), so a string
                # here is a name collision with the free-form vocabulary and not a
                # value; silently filing it in the payload would leave the column
                # NULL for a stage that believes it reported the counter.
                raise TypeError(
                    f"promoted counter {name!r} takes an int, not {type(value).__name__}"
                )
            self.promote(name, value)
            return
        self._extra[name] = value

    def column(self, name: str) -> int | None:
        """The value of promoted counter ``name``, or ``None`` if unwritten.

        Raises:
            UnknownCounterError: ``name`` is not one of the eleven (S5.2).
        """
        if name not in PROMOTED_COUNTERS:
            raise UnknownCounterError(name)
        return self._promoted.get(name)

    def columns(self) -> tuple[int | None, ...]:
        """Every promoted counter in :data:`PROMOTED_COUNTERS` order, NULL for unset.

        S5.2: "a stage writes NULL for counters that do not apply to it".
        """
        return tuple(self._promoted.get(name) for name in PROMOTED_COUNTERS)

    def payload(self) -> dict[str, object]:
        """The `run_stages.counters` object: the S5.2 union, in a stable order.

        Declared names first, in the order S4 lists them, then the promoted counters
        this stage wrote, then anything else it recorded. The order is stable so two
        runs of the same stage produce byte-identical JSON, which is what lets a diff
        of two `run_stages` rows mean something.
        """
        merged: dict[str, object] = {name: None for name in self.declared}
        for name in PROMOTED_COUNTERS:
            if name in self._promoted:
                merged[name] = self._promoted[name]
        merged.update(self._extra)
        return merged

    def payload_json(self) -> str:
        """:meth:`payload` as the compact JSON the `counters` column stores."""
        return json.dumps(self.payload(), separators=(",", ":"), ensure_ascii=False)
