"""Distribute the complete integration collection across isolated CI jobs."""

from __future__ import annotations

import pytest


def pytest_addoption(parser: pytest.Parser) -> None:
    group = parser.getgroup("integration sharding")
    group.addoption("--integration-shard-index", type=int, default=None)
    group.addoption("--integration-shard-count", type=int, default=None)


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line(
        "markers", "shard_together: keep this module's ordered checks in one CI job"
    )


@pytest.hookimpl(trylast=True)
def pytest_collection_modifyitems(config: pytest.Config, items: list[pytest.Item]) -> None:
    # Run after marker filtering so every job partitions the same eligible tests.
    index = config.getoption("integration_shard_index")
    count = config.getoption("integration_shard_count")
    if index is None and count is None:
        return
    if index is None or count is None or count < 1 or not 0 <= index < count:
        raise pytest.UsageError(
            "integration sharding requires both options with 0 <= index < count"
        )
    if config.getoption("numprocesses", default=0):
        raise pytest.UsageError("integration shards run serially in separate Compose stacks")

    groups: dict[str, list[pytest.Item]] = {}
    for item in items:
        key = (
            f"module:{item.path}"
            if item.get_closest_marker("shard_together") is not None
            else f"case:{item.nodeid}"
        )
        groups.setdefault(key, []).append(item)
    loads = [0] * count
    selected_ids: set[int] = set()
    for key in sorted(groups, key=lambda key: (-len(groups[key]), key)):
        target = min(range(count), key=lambda shard: (loads[shard], shard))
        loads[target] += len(groups[key])
        if target == index:
            selected_ids.update(id(item) for item in groups[key])
    # Preserve pytest's source order, including producer/consumer fixture checks.
    selected = [item for item in items if id(item) in selected_ids]
    deselected = [item for item in items if id(item) not in selected_ids]
    config.hook.pytest_deselected(items=deselected)
    items[:] = selected
