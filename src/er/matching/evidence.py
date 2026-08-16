"""The `evidence` / `waterfall` payload: what a scored pair carries with it (S4.3.5).

S4.3.5 is one sentence long about this and every word of it is load-bearing —
"`waterfall JSON` retains the `gamma_*` comparison-vector columns and per-comparison
Bayes factors from `predict()`; they MUST be retained rather than projected away".
The payload it describes is written twice per pair: into `match_scores.evidence` for
every scored pair (S5), and into `review_queue.waterfall` for the gray-band subset.
Both are the same object, built here, so a steward reading a queued pair and an
operator reading the score behind it are looking at one record.

**What "retained" costs, and why it is not optional.** The gamma vector says which
comparison level each attribute landed on; the Bayes factor says what that level was
worth. Neither is recoverable from `match_probability` afterwards — the probability is
their product through the model, and a product cannot be factored back — so a payload
stripped to the probability leaves a reviewer with a number and no reason. The failure
is silent and permanent: the pair is scored, the row is written, and the evidence for
it no longer exists anywhere.

**The keys are a function of the config, not a fixed list.** Splink names its output
columns after each comparison's `output_column_name`, which
:func:`er.matching.model.build_settings` sets to the `comparisons:` key (S4.3.1), so
the payload's `gamma_*` suffixes are exactly that key set. A TF-adjusted comparison
carries a third column — `bf_tf_adj_<column>`, the term-frequency correction applied
on top of the level's own Bayes factor — and it is retained too: under D4 the
adjustment is read from a *frozen* `tf_lookup`, so it is part of what makes a
re-derivation of the score reproducible.

The prefixes themselves are imported from :mod:`er.review.queue`, which validates the
payload it is handed (a gray-band waterfall carrying no `gamma_*` key or no Bayes
factor is refused there). Producer and validator agreeing on the spelling of a prefix
is the whole of that check, so there is one spelling.
"""

from __future__ import annotations

import re
from collections.abc import Collection
from typing import Final

from er.config.schema import Config
from er.errors import StageFailure
from er.review.queue import BAYES_FACTOR_PREFIX, GAMMA_PREFIX

__all__ = [
    "BAYES_FACTOR_PREFIX",
    "EVIDENCE_KEYS",
    "GAMMA_PREFIX",
    "MATCH_WEIGHT_KEY",
    "TF_ADJUSTMENT_PREFIX",
    "build_evidence",
    "evidence_keys",
]

#: How Splink names the term-frequency correction of a TF-adjusted comparison. It sits
#: beside `bf_<column>` rather than inside it: the level's Bayes factor is the model's,
#: the adjustment is the corpus's, and D4 freezes only the second.
TF_ADJUSTMENT_PREFIX: Final = "bf_tf_adj_"

#: The one whole-pair key. `match_weight` is `log2` of the product of every Bayes
#: factor in the payload, so it is what the rest of the object has to add up to — the
#: reason it is retained beside them rather than derived on read.
MATCH_WEIGHT_KEY: Final = "match_weight"

#: Every key family an evidence payload is built from, in payload order: three
#: per-comparison prefixes that :func:`evidence_keys` expands against a config, then
#: the one key that is already whole. Declared as a tuple so a reader of S4.3.5 can
#: check the section against a single line.
EVIDENCE_KEYS: Final[tuple[str, ...]] = (
    GAMMA_PREFIX,
    BAYES_FACTOR_PREFIX,
    TF_ADJUSTMENT_PREFIX,
    MATCH_WEIGHT_KEY,
)

#: A column name safe to interpolate into the `json_object(...)` call below. Every key
#: this module builds is a prefix plus a `comparisons:` key, and a `comparisons:` key
#: is validated to be a column of `int_std_records` (S6.1 V6) — but the check is made
#: here anyway, because this is where a config-authored string reaches SQL as an
#: identifier rather than as a parameter.
_IDENTIFIER: Final = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def evidence_keys(cfg: Config, available: Collection[str]) -> tuple[str, ...]:
    """The payload's keys for one config, restricted to what `predict()` emitted.

    Args:
        cfg: the validated S6 document; only `comparisons:` is read.
        available: the column names of the prediction relation. Passed in rather than
            assumed, because the third key of a TF-adjusted comparison exists only if
            the *model* carries the adjustment, and the model is a frozen artifact
            this stage loads rather than builds.

    Returns:
        ``gamma_<column>``, ``bf_<column>`` and — where the model emitted it —
        ``bf_tf_adj_<column>`` for each `comparisons:` key in config order, then
        :data:`MATCH_WEIGHT_KEY`.

    Raises:
        er.errors.StageFailure: a `gamma_*`, a `bf_*` or the match weight is missing
            from ``available``. That is not a payload to be built with a hole in it:
            S4.3.5 requires the vector and the factors to be retained, and a settings
            document with `retain_intermediate_calculation_columns` off produces
            exactly this shape (see :func:`er.matching.full.score_full`, which turns it
            on before scoring).
    """
    present = frozenset(available)
    keys: list[str] = []
    missing: list[str] = []
    for column, spec in cfg.comparisons.items():
        for key in (f"{GAMMA_PREFIX}{column}", f"{BAYES_FACTOR_PREFIX}{column}"):
            (keys if key in present else missing).append(key)
        # Optional, and silently so: `tf: true` says the config asked for the
        # adjustment, and the frozen model says whether it was fitted with one.
        adjustment = f"{TF_ADJUSTMENT_PREFIX}{column}"
        if spec.tf and adjustment in present:
            keys.append(adjustment)
    (keys if MATCH_WEIGHT_KEY in present else missing).append(MATCH_WEIGHT_KEY)
    if missing:
        raise StageFailure(
            f"the prediction carries none of {', '.join(missing)}; S4.3.5 requires the "
            f"gamma_* comparison vector and the per-comparison Bayes factors to be retained "
            f"rather than projected away. The relation has {', '.join(sorted(present))}"
        )
    return tuple(keys)


def build_evidence(cfg: Config, available: Collection[str]) -> str:
    """The SQL expression that renders one prediction row's evidence as JSON.

    An expression rather than a Python dict, and that is what makes S4.3.4's "a single
    write statement" reachable: the payload is built by the same `MERGE INTO` that
    persists the pair, over the columns of the prediction relation, so no scored pair
    is ever materialized in Python on the way to the lake.

    Args:
        cfg: the validated S6 document.
        available: the column names of the prediction relation.

    Returns:
        ``json_object('gamma_<column>', gamma_<column>, …, 'match_weight',
        match_weight)`` — an expression valid wherever those columns are in scope.

    Raises:
        er.errors.StageFailure: a required key is missing (see :func:`evidence_keys`),
            or one is not a bare identifier. The second cannot happen through a
            validated config and is checked because the keys are interpolated.
    """
    keys = evidence_keys(cfg, available)
    for key in keys:
        if not _IDENTIFIER.fullmatch(key):
            raise StageFailure(
                f"{key!r} is not a bare identifier and cannot be interpolated into the "
                f"evidence expression; a `comparisons:` key is a column of int_std_records "
                f"(S6.1 V6)"
            )
    arguments = ", ".join(f"'{key}', {key}" for key in keys)
    return f"json_object({arguments})"
