"""The S5.2 counter split: eleven typed columns, everything else in JSON (S8.4).

The eleven names are written out as a literal here rather than imported from the
registry, because the claim under test is that the registry, the constant and the
spec all say the same thing — and a test that derived its expectation from one of
them could not catch the other two being wrong.
"""

from __future__ import annotations

import json

import pytest

from er.lake.model import REGISTRY
from er.obs.counters import (
    DECLARED_COUNTERS,
    PROMOTED_COUNTERS,
    StageCounters,
    UnknownCounterError,
)

# S5.2's promoted list, in its order.
S5_2_PROMOTED = (
    "rows_in",
    "rows_out",
    "candidate_pairs",
    "pairs_above_auto_merge",
    "entities_created",
    "entities_merged",
    "entities_split",
    "entities_retired",
    "edges_cut",
    "review_queue_added",
    "duration_ms",
)

# `run_stages`' other BIGINT columns. They are structure — the stage's position in
# the run and the range it committed — and not counters, which is exactly the
# distinction "the eleven typed counter columns" makes.
STRUCTURAL_BIGINT = ("seq", "snapshot_start", "snapshot_end")


def run_stages_counter_columns() -> tuple[str, ...]:
    """The BIGINT columns of `run_stages` that are counters, in DDL order."""
    spec = REGISTRY["run_stages"]
    return tuple(
        column.name
        for column in spec.columns
        if column.type == "BIGINT" and column.name not in STRUCTURAL_BIGINT
    )


def test_promoted_counters_equal_run_stages_typed_columns() -> None:
    """AC4: the constant, the registry and S5.2 agree, in both directions."""
    columns = run_stages_counter_columns()

    assert PROMOTED_COUNTERS == S5_2_PROMOTED
    assert columns == S5_2_PROMOTED
    # Both directions spelled out: neither set may hold a name the other does not.
    assert set(PROMOTED_COUNTERS) - set(columns) == set()
    assert set(columns) - set(PROMOTED_COUNTERS) == set()
    assert len(PROMOTED_COUNTERS) == 11

    # Every promoted counter is nullable: a stage writes NULL for the ones that do
    # not apply to it (S5.2), so a NOT NULL here would make the closed set unusable.
    nullable = {column.name: column.nullable for column in REGISTRY["run_stages"].columns}
    assert all(nullable[name] for name in PROMOTED_COUNTERS)


def test_unknown_counter_goes_to_json_not_column() -> None:
    """AC4: an unlisted name is refused as a column and accepted in the payload."""
    counters = StageCounters(declared=("files",))

    with pytest.raises(UnknownCounterError, match="files"):
        counters.promote("files", 3)
    with pytest.raises(UnknownCounterError):
        counters.column("files")
    assert "files" not in REGISTRY["run_stages"].column_names

    counters.set("files", 3)
    assert counters.payload()["files"] == 3
    # Routed, not merely tolerated: a promoted name still reaches its column.
    counters.set("rows_in", 7)
    assert counters.column("rows_in") == 7
    assert counters.columns()[PROMOTED_COUNTERS.index("rows_in")] == 7

    # A promoted counter is a BIGINT column, so a string under one of those names is
    # a collision with the free-form vocabulary rather than a value.
    with pytest.raises(TypeError):
        counters.set("rows_out", "many")


def test_counters_payload_is_union_of_declared_and_promoted() -> None:
    """AC5: the payload is (a) the S4-declared names ∪ (b) the promoted ones written."""
    declared = DECLARED_COUNTERS["assemble"]
    counters = StageCounters(declared)
    counters.set("entities_rebuilt", 4)  # declared, free-form
    counters.set("rows_in", 9)  # promoted, NOT declared by S4.6
    counters.promote("duration_ms", 12)

    payload = json.loads(counters.payload_json())

    assert set(payload) == set(declared) | {"rows_in"}
    assert payload["entities_rebuilt"] == 4
    assert payload["rows_in"] == 9
    assert payload["duration_ms"] == 12
    # A declared name the stage never measured is still in the record, as null: the
    # payload is a complete statement about the stage, not only about what it counted.
    assert payload["lineage_rows"] is None

    # And a payload that dropped a declared name fails the completeness rule, which
    # is what the assertion above is checking for every stage in the chain.
    incomplete = {name: value for name, value in payload.items() if name != "lineage_rows"}
    assert not set(incomplete) >= set(declared)


def test_declared_counters_cover_the_five_stages_s4_lists() -> None:
    """AC5: the per-stage vocabulary comes from the S4 subsections, verbatim."""
    assert set(DECLARED_COUNTERS) == {
        "ingest",
        "standardize",
        "match",
        "reconcile",
        "assemble",
    }
    for stage, names in DECLARED_COUNTERS.items():
        assert "duration_ms" in names, f"S4 ends {stage}'s list with duration_ms"
        assert len(set(names)) == len(names), f"{stage} declares a name twice"
        # S5.2: the S4 lists deliberately omit `rows_in`/`rows_out`, which the same
        # paragraph states separately and the writer adds.
        assert "rows_in" not in names
        assert "rows_out" not in names
