"""The phase-2 coherence seam (S11, M20): the interface only, `noop` in v1.

S11 reserves a place for an embedding-based cluster-coherence check without shipping any
embedding: a `CoherenceScorer` is constructed once per run, scores the run's rebuilt
entities, and its findings above a threshold land as `review_queue` rows with
`subject_type='entity'` (the entity-scoped half of M20). v1 registers only `NoopScorer`,
which reports zero dispersion for every cluster and therefore writes nothing — so the seam
runs on every `er assemble` without changing a single output row (M2).

Three things are load-bearing:

* **`CoherenceScorer` is a structural `typing.Protocol`**, so `NoopScorer` and a test
  double satisfy it without any runtime base class (S11, AC7) and `mypy --strict` proves
  it over the package.
* **`score_clusters` returns one result per input, in input order**, because the caller
  mints review ids in that order and the run must be reproducible (S11).
* **An unknown `coherence.scorer` is a config error (exit 2)**, never a silent fallback to
  `noop`: a misconfigured scorer that scored nothing would hide the misconfiguration.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Protocol

from er.config.schema import Config
from er.errors import ConfigError

__all__ = [
    "SCORER_REGISTRY",
    "ClusterCoherence",
    "CoherenceScorer",
    "NoopScorer",
    "get_scorer",
    "register_scorer",
]


@dataclass(frozen=True)
class ClusterCoherence:
    """One cluster's coherence (S11): its `entity_id`, a dispersion score, and the record
    keys the scorer judged outliers. A finding is written only when `dispersion` exceeds
    the scorer's threshold."""

    entity_id: str
    dispersion: float
    outlier_record_keys: tuple[str, ...] = ()


class CoherenceScorer(Protocol):
    """The S11 scorer interface: a dispersion threshold and a batch scoring call.

    Structural on purpose (AC7) — an implementation is a `CoherenceScorer` by having these
    members, with no registration as a subclass.
    """

    @property
    def threshold(self) -> float:
        """Findings with `dispersion` strictly above this are queued (S11)."""

    def score_clusters(self, entity_ids: Sequence[str]) -> list[ClusterCoherence]:
        """Score each entity, returning one :class:`ClusterCoherence` per input id, in the
        same order (S11)."""


@dataclass(frozen=True)
class NoopScorer:
    """v1's scorer: zero dispersion and no outliers for every cluster (S11, S1).

    It writes no review rows — its dispersion never exceeds the threshold — so the seam
    executes on every run and changes nothing. The threshold is ``1.0`` so that a scorer
    which (unlike this one) reported a unit-scaled dispersion would have a meaningful
    boundary; `NoopScorer` itself never reaches it.
    """

    threshold: float = 1.0

    def score_clusters(self, entity_ids: Sequence[str]) -> list[ClusterCoherence]:
        return [ClusterCoherence(entity_id=entity_id, dispersion=0.0) for entity_id in entity_ids]


#: `coherence.scorer` name -> factory. A factory takes the validated config so a scorer
#: can read its own sub-block; `noop` reads nothing.
ScorerFactory = Callable[[Config], CoherenceScorer]
SCORER_REGISTRY: dict[str, ScorerFactory] = {}


def register_scorer(name: str, factory: ScorerFactory) -> None:
    """Register `factory` under `name` for `coherence.scorer` selection (S11, S6)."""
    SCORER_REGISTRY[name] = factory


def get_scorer(cfg: Config) -> CoherenceScorer:
    """Construct the scorer `coherence.scorer` names (S11, S6).

    Raises:
        er.errors.ConfigError: the configured name is not registered (exit 2, S4.0). The
            message names the unknown value and every registered name, never falling back
            to `noop`.
    """
    name = cfg.coherence.scorer
    factory = SCORER_REGISTRY.get(name)
    if factory is None:
        registered = ", ".join(sorted(SCORER_REGISTRY)) or "(none registered)"
        raise ConfigError(
            f"coherence.scorer: {name!r} is not a registered scorer; registered: {registered}"
        )
    return factory(cfg)


register_scorer("noop", lambda _cfg: NoopScorer())
