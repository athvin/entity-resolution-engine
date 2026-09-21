"""Distribute the complete integration collection across isolated CI jobs."""

from __future__ import annotations

import pytest


def pytest_addoption(parser: pytest.Parser) -> None:
    group = parser.getgroup("integration sharding")
    group.addoption("--integration-shard-index", type=int, default=None)
    group.addoption("--integration-shard-count", type=int, default=None)


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

    selected = sorted(items, key=lambda item: item.nodeid)[index::count]
    selected_ids = {id(item) for item in selected}
    deselected = [item for item in items if id(item) not in selected_ids]
    config.hook.pytest_deselected(items=deselected)
    items[:] = selected
