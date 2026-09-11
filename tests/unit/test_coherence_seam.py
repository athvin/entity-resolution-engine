"""Unit arm of the S11 coherence seam (ER-104): NoopScorer behaviour, the registry, the
unknown-name config error, and the structural Protocol. No lake — the hook is exercised by
tests/integration/test_coherence_hook.py.
"""

from __future__ import annotations

from pathlib import Path

import pytest
from helpers.config_mutation import mutated_config
from helpers.scorers import FakeScorer

from er.config.loader import load_config
from er.embeddings.coherence import (
    ClusterCoherence,
    CoherenceScorer,
    NoopScorer,
    get_scorer,
)
from er.errors import ERROR_CLASS_TO_EXIT, ConfigError, ExitCode

REPO_ROOT = Path(__file__).resolve().parents[2]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"


def test_noop_scorer_returns_zero_dispersion_in_input_order() -> None:
    """AC1: one result per input, in input order, with zero dispersion and no outliers."""
    results = NoopScorer().score_clusters(["E2", "E1"])
    assert [c.entity_id for c in results] == ["E2", "E1"]
    assert all(isinstance(c, ClusterCoherence) for c in results)
    assert all(c.dispersion == 0.0 and c.outlier_record_keys == () for c in results)


def test_registry_returns_noop_by_default() -> None:
    """AC1: the committed config selects `noop`, and get_scorer returns a NoopScorer."""
    cfg = load_config(TEST_CONFIG)
    assert cfg.coherence.scorer == "noop"
    assert isinstance(get_scorer(cfg), NoopScorer)


def test_unknown_scorer_name_is_config_error_exit_2(tmp_path: Path) -> None:
    """AC2: an unregistered coherence.scorer is a config error (exit 2) naming the value
    and the registered names — never a silent fallback to noop."""
    mutated = mutated_config(TEST_CONFIG, {"coherence.scorer": "does_not_exist"}, dest_dir=tmp_path)
    cfg = load_config(mutated)
    with pytest.raises(ConfigError) as raised:
        get_scorer(cfg)
    assert ERROR_CLASS_TO_EXIT[raised.value.error_class] == ExitCode.CONFIG
    message = str(raised.value)
    assert "does_not_exist" in message and "noop" in message


def test_noop_satisfies_protocol_structurally() -> None:
    """AC7: NoopScorer and FakeScorer are CoherenceScorers structurally — a function typed
    to take the Protocol accepts them with no subclass registration (mypy proves the
    static half; this proves the call works)."""

    def accepts(scorer: CoherenceScorer) -> list[ClusterCoherence]:
        return scorer.score_clusters(["E1"])

    assert accepts(NoopScorer())[0].entity_id == "E1"
    fake = FakeScorer(threshold=0.5, dispersion_by_entity={"E1": 0.9}, outliers_by_entity={})
    assert accepts(fake)[0].dispersion == 0.9
