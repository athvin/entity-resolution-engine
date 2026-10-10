"""The §6.5 resource classes: boundaries, invariants, and the override lever.

Pure, like the retry matrix: no cluster, no database. The two invariants are
docs/infrastructure.md §6.5's own — threads must match the CPU request set in
the same operation, and the DuckDB memory limit must sit strictly below the
pod memory limit (the OOM killer versus a graceful spill).
"""

from __future__ import annotations

import pytest
from erserver.sizing import (
    CLASS_L,
    CLASS_M,
    CLASS_S,
    DEFAULT_CLASS,
    RESOURCE_CLASSES,
    ResourceClass,
    class_named,
    select_class,
)


def _gibibytes(quantity: str) -> float:
    assert quantity.endswith("Gi"), f"pod quantities here are Gi, got {quantity!r}"
    return float(quantity[: -len("Gi")])


def _duckdb_gigabytes(quantity: str) -> float:
    assert quantity.endswith("GB"), f"DuckDB limits here are GB, got {quantity!r}"
    return float(quantity[: -len("GB")])


def test_the_table_is_section_6_5_verbatim() -> None:
    assert (CLASS_S.cpu, CLASS_S.memory, CLASS_S.duckdb_threads) == ("2", "6Gi", 2)
    assert (CLASS_S.duckdb_memory_limit, CLASS_S.ephemeral_storage) == ("4GB", "40Gi")
    assert (CLASS_M.cpu, CLASS_M.memory, CLASS_M.duckdb_threads) == ("6", "10Gi", 6)
    assert (CLASS_M.duckdb_memory_limit, CLASS_M.ephemeral_storage) == ("6GB", "150Gi")
    assert (CLASS_L.memory, CLASS_L.duckdb_memory_limit) == ("14Gi", "8GB")
    assert CLASS_L.ephemeral_storage == "400Gi"
    # §6.5 gives L a 12-16 vCPU band; whatever point is chosen, cpu == threads.
    assert 12 <= CLASS_L.duckdb_threads <= 16


@pytest.mark.parametrize("cls", RESOURCE_CLASSES, ids=lambda cls: str(cls.name))
def test_threads_and_cpu_move_together(cls: ResourceClass) -> None:
    assert int(cls.cpu) == cls.duckdb_threads


@pytest.mark.parametrize("cls", RESOURCE_CLASSES, ids=lambda cls: str(cls.name))
def test_duckdb_limit_strictly_below_pod_memory(cls: ResourceClass) -> None:
    # 1 GB < 1 Gi, so comparing the raw numbers is conservative in the safe
    # direction: if GB-number < Gi-number holds, bytes hold too.
    assert _duckdb_gigabytes(cls.duckdb_memory_limit) < _gibibytes(cls.memory)


@pytest.mark.parametrize("cls", RESOURCE_CLASSES, ids=lambda cls: str(cls.name))
def test_deadlines_and_pools_are_the_6_2_values(cls: ResourceClass) -> None:
    assert {"S": 7200, "M": 21600, "L": 86400}[cls.name] == cls.active_deadline_seconds
    assert cls.nodepool == ("runner-l" if cls.name == "L" else "runner-sm")


def test_class_boundaries_are_inclusive_at_the_top() -> None:
    assert select_class(1).name == "S"
    assert select_class(100_000).name == "S"
    assert select_class(100_001).name == "M"
    assert select_class(1_000_000).name == "M"
    assert select_class(1_000_001).name == "L"
    assert select_class(10_000_000).name == "L"
    # Above the ladder is still L: the ladder tops out, it never refuses.
    assert select_class(50_000_000).name == "L"


def test_unknown_record_count_takes_the_default_class() -> None:
    assert select_class(None) is DEFAULT_CLASS
    assert DEFAULT_CLASS is CLASS_M


def test_provision_is_lifecycle_work_and_always_small() -> None:
    assert select_class(None, kind="provision") is CLASS_S
    assert select_class(10_000_000, kind="provision") is CLASS_S


def test_params_override_wins_and_a_typo_is_ignored() -> None:
    assert select_class(10, override="L") is CLASS_L
    assert select_class(10_000_000, override="S") is CLASS_S
    # An operator typo must not fail the tenant's run.
    assert select_class(10_000_000, override="XXL").name == "L"
    assert select_class(None, override=7).name == DEFAULT_CLASS.name


def test_class_named_round_trips_every_class() -> None:
    for cls in RESOURCE_CLASSES:
        assert class_named(cls.name) is cls
    assert class_named("nope") is None
