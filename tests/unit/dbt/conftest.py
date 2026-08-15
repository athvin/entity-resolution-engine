"""Fixtures for the dbt macro unit layer (DesignDoc.md S8.1).

The macros under test read their settings from the `standardization` var, so the
payload they are rendered against is built the way the pipeline builds it -- by
`render_dbt_vars` from `configs/test.yaml` -- and never hand-written here. That is
what makes S4.2's "the config is the single source of truth" a property of the
tests as well as of the macros: change `configs/test.yaml` and these tests change
with it, which is exactly the coupling S6 asks for.

The harness is session-scoped. It holds a Jinja environment and one in-process
DuckDB connection, both stateless with respect to a single macro render, and
`hypothesis` rejects a function-scoped fixture underneath `@given` anyway. A test
that mutates harness state -- registering a seed or a relation -- builds its own.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from harness import MacroHarness

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import render_dbt_vars

#: `configs/test.yaml` is the document S6 says the fixtures and CI use verbatim.
#: Absolute, so the suite does not depend on the working directory it is run from.
TEST_CONFIG_PATH = Path(__file__).resolve().parents[3] / "configs" / "test.yaml"

#: Every dbt invocation carries a `run_id` (S6). No macro here reads it; it is a
#: literal rather than a fresh ULID so a render is reproducible across sessions.
RUN_ID = "01JZZZZZZZZZZZZZZZZZZZZZZZ"


@pytest.fixture(scope="session")
def test_config() -> Config:
    return load_config(TEST_CONFIG_PATH)


@pytest.fixture(scope="session")
def dbt_vars(test_config: Config) -> dict[str, Any]:
    """The `--vars` payload the CLI would pass, including `standardization`."""
    return render_dbt_vars(test_config, RUN_ID)


@pytest.fixture(scope="session")
def harness(dbt_vars: dict[str, Any]) -> Iterator[MacroHarness]:
    macro_harness = MacroHarness(vars=dbt_vars)
    try:
        yield macro_harness
    finally:
        macro_harness.close()
