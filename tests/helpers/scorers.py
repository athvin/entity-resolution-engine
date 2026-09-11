"""A registrable `CoherenceScorer` test double for the S11 seam (ER-104).

`FakeScorer` records how many times it was constructed and called and the id list it
received, and returns a caller-specified dispersion per entity — so a test can both drive
a finding above the threshold and assert the hook constructs/scores exactly once with the
run's rebuilt ids. State lives on a module-level recorder because the assemble hook
constructs the scorer internally (through `get_scorer`), so the test cannot hold the
instance; `register_fake` resets the recorder and registers the factory under a name.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field

from er.config.schema import Config
from er.embeddings.coherence import ClusterCoherence, register_scorer

__all__ = ["FAKE_STATE", "FakeScorer", "register_fake"]


@dataclass
class _FakeState:
    """What the hook did to the fake, observed across its internal construction."""

    constructions: int = 0
    calls: list[list[str]] = field(default_factory=list)

    def reset(self) -> None:
        self.constructions = 0
        self.calls = []


#: The shared recorder a test reads after the hook has run.
FAKE_STATE = _FakeState()


@dataclass(frozen=True)
class FakeScorer:
    """A scorer with a fixed per-entity dispersion, recording every call to FAKE_STATE."""

    threshold: float
    dispersion_by_entity: Mapping[str, float]
    outliers_by_entity: Mapping[str, Sequence[str]]

    def score_clusters(self, entity_ids: Sequence[str]) -> list[ClusterCoherence]:
        FAKE_STATE.calls.append(list(entity_ids))
        return [
            ClusterCoherence(
                entity_id=entity_id,
                dispersion=self.dispersion_by_entity.get(entity_id, 0.0),
                outlier_record_keys=tuple(self.outliers_by_entity.get(entity_id, ())),
            )
            for entity_id in entity_ids
        ]


def register_fake(
    name: str = "fake",
    *,
    threshold: float,
    dispersion_by_entity: Mapping[str, float],
    outliers_by_entity: Mapping[str, Sequence[str]] | None = None,
) -> _FakeState:
    """Reset the recorder and register a `FakeScorer` factory under `name`.

    Returns the recorder so the test can assert the construction/call counts afterward.
    """
    FAKE_STATE.reset()
    resolved_outliers = outliers_by_entity or {}

    def factory(_cfg: Config) -> FakeScorer:
        FAKE_STATE.constructions += 1
        return FakeScorer(threshold, dispersion_by_entity, resolved_outliers)

    register_scorer(name, factory)
    return FAKE_STATE
