"""`cluster_full` passes Splink's clustering threshold explicitly (S4.3, D13).

Splink's ``cluster_pairwise_predictions_at_threshold`` declares
``threshold_match_probability: Optional[float] = None``, and a ``None`` threshold
treats **every supplied edge as a match**. So the difference between a correct
clustering call and one that silently merges the entire gray band is a single keyword
argument that no result inspection can recover: given an edge set that happens to hold
nothing below `auto_merge`, both spellings return the same partition.

That is why this module spies on the call rather than on the answer. MINOR-thresholds
and D13 pin the clustering cut to `thresholds.auto_merge`, and the argument
:func:`~er.entities.cluster.cluster_full` passes is the only place that pin exists in
executable form.

The two tests are deliberately separate. One asserts the argument is *present* — the
failure mode where someone deletes the kwarg and Splink's default quietly takes over.
The other asserts its *value* comes from the validated config rather than from a
literal, which is M26: a hard-coded `0.95` would keep passing here and be wrong for
every tenant whose document says otherwise.

These run on a bare `duckdb.connect()`. `cluster_full` reads no lake relation — it
materialises its two inputs on whatever connection it is handed — so the unit layer can
exercise the real function against the real Splink rather than a mock of it.
"""

from __future__ import annotations

import os
from collections.abc import Iterator
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import fixture_settings
from splink.internals.linker_components.clustering import LinkerClustering

from er.config.loader import load_config
from er.config.schema import Config
from er.entities.cluster import Edge, cluster_full

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
TEST_CONFIG_PATH: Final = REPO_ROOT / "configs" / "test.yaml"

#: `splink_api` pins DuckDB's thread count from the environment (S7.1). The unit layer
#: runs on a bare runner with no Compose envelope, so the variable is supplied here.
THREADS_ENV: Final = "ER_DUCKDB_THREADS"

#: The keyword whose absence is the defect this module exists to catch.
THRESHOLD_KWARG: Final = "threshold_match_probability"


@pytest.fixture
def cfg() -> Config:
    """`configs/test.yaml` — the document S6 says the fixtures and CI use verbatim."""
    return load_config(TEST_CONFIG_PATH)


@pytest.fixture
def connection(monkeypatch: pytest.MonkeyPatch) -> Iterator[duckdb.DuckDBPyConnection]:
    """A bare in-memory connection, with the one environment variable `splink_api` reads."""
    monkeypatch.setenv(THREADS_ENV, os.environ.get(THREADS_ENV, "2"))
    handle = duckdb.connect()
    try:
        yield handle
    finally:
        handle.close()


@pytest.fixture
def threshold_spy(monkeypatch: pytest.MonkeyPatch) -> list[dict[str, Any]]:
    """Every keyword argument the clustering call received, in call order."""
    calls: list[dict[str, Any]] = []
    original = LinkerClustering.cluster_pairwise_predictions_at_threshold

    def recording(self: LinkerClustering, *args: Any, **kwargs: Any) -> Any:
        calls.append(dict(kwargs))
        return original(self, *args, **kwargs)

    monkeypatch.setattr(LinkerClustering, "cluster_pairwise_predictions_at_threshold", recording)
    return calls


def two_record_edge_set() -> tuple[list[str], list[Edge]]:
    """The smallest input that reaches the clustering call: two nodes, one edge."""
    return (
        ["crm:C1", "crm:C2"],
        [Edge(rec_a_key="crm:C1", rec_b_key="crm:C2", match_probability=0.99)],
    )


def test_cluster_call_passes_explicit_threshold(
    connection: duckdb.DuckDBPyConnection, cfg: Config, threshold_spy: list[dict[str, Any]]
) -> None:
    """AC2: the threshold reaches Splink as a keyword, not as its `None` default.

    A `None` threshold clusters every supplied edge regardless of probability, so
    omitting this argument would merge the gray band into the partition and no
    assertion about the returned components would necessarily notice (S4.3, D13).
    """
    nodes, edges = two_record_edge_set()
    cluster_full(
        connection,
        nodes,
        edges,
        auto_merge=cfg.thresholds.auto_merge,
        settings=fixture_settings(),
    )

    assert threshold_spy, "cluster_full never reached Splink's clustering call"
    assert len(threshold_spy) == 1, f"{len(threshold_spy)} clustering calls, expected one"
    assert THRESHOLD_KWARG in threshold_spy[0], (
        f"the clustering call was made without {THRESHOLD_KWARG!r}; Splink's default is "
        f"None, which treats every supplied edge as a match (S4.3, D13). Keywords "
        f"passed: {sorted(threshold_spy[0])}"
    )
    assert threshold_spy[0][THRESHOLD_KWARG] is not None, (
        f"{THRESHOLD_KWARG} was passed explicitly as None, which is the same defect "
        "spelled differently"
    )


def test_threshold_value_comes_from_config_auto_merge(
    connection: duckdb.DuckDBPyConnection, cfg: Config, threshold_spy: list[dict[str, Any]]
) -> None:
    """AC2: the value is `thresholds.auto_merge` from the validated document (M26).

    Asserted against the config rather than against `0.95` so that a tenant whose
    document says something else is covered by this test too — a literal here would
    pass while the clustering cut disagreed with the scoring cut.
    """
    nodes, edges = two_record_edge_set()
    cluster_full(
        connection,
        nodes,
        edges,
        auto_merge=cfg.thresholds.auto_merge,
        settings=fixture_settings(),
    )

    assert threshold_spy
    passed = threshold_spy[0][THRESHOLD_KWARG]
    assert passed == cfg.thresholds.auto_merge, (
        f"the clustering cut is {passed!r} but thresholds.auto_merge is "
        f"{cfg.thresholds.auto_merge!r}; S4.3 says the clustering threshold IS "
        "auto_merge, so the two cannot differ"
    )
    # The other half of D13: the cut is a probability, not a match weight. Passing a
    # weight into a probability parameter is the failure `er.matching.full` documents
    # for `predict`, and it is available here in exactly the same shape.
    assert 0.0 <= float(passed) <= 1.0, f"{passed!r} is not a probability"
