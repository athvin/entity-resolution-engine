"""The S4.3.2 training sequence: declared once, executed from the declaration.

`er train` runs full-corpus EM, and S4.3.2 fixes both the order of the Splink calls
and where every argument comes from — the `training:` block of S6, which Pydantic
rejects if incomplete. M8 is what that fixed sequence closes: two of the three
estimators have required arguments the old config could not supply, and
`estimate_u_using_random_sampling` was unseeded, so two trainings over one corpus
produced two different models and T-TRAIN-1 was unachievable.

The sequence is therefore held as data — :data:`TRAIN_CALL_SEQUENCE` — rather than as
three hand-written call sites. :func:`train_model` renders it against a `Config` and
invokes what it rendered, so the declaration a reader (and the spy test) checks is the
same object the stage executes; a call site that drifted from the declaration is not
expressible. `tests/unit/matching/test_train_sequence.py` checks the declaration
itself against the fenced Python block under S4.3.2, so the spec is the authority for
both the order and the argument sources rather than this module being its own.

Three constraints are easy to miss:

* Splink 4's estimators are namespaced (`linker.training.*`). A call on the bare
  linker is the Splink 3 API, which is why the declared method path carries the
  namespace and is walked attribute by attribute.
* `seed` is not optional here even though it is in Splink (its default is ``None``).
  An unseeded u estimate makes the settings JSON a different document on every run.
* At least two EM sessions run, because m is not estimated for a blocked column
  (V9) — one session would leave the blocked column's m at its prior.

This module is deliberately short of `er train`: `model_version` allocation, the S3
artifact upload, the `model_registry` row, the active/superseded pointer and
`run_stages` are ER-055's. What it does own beyond the sequence is the `tf_lookup`
materialization, because D4 permits `er train` to mint a `tf_snapshot_id` and freeze
TF under it, and confines that to two callers of which this is the first.
"""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass, field
from functools import reduce
from typing import Any, Final, Protocol

import duckdb
from splink import Linker

from er.config.schema import Config
from er.errors import StageFailure
from er.matching.api import splink_api
from er.matching.model import build_settings
from er.matching.tf import materialize_tf_lookup, new_tf_snapshot_id, tf_columns

__all__ = [
    "EM_RULE",
    "NON_FITTED_SETTINGS_KEYS",
    "SETTINGS_READ_PATH",
    "TRAIN_CALL_SEQUENCE",
    "TrainCall",
    "TrainCallSpec",
    "TrainLinker",
    "TrainResult",
    "build_training_linker",
    "render_call_sequence",
    "train_model",
]

#: The one argument in S4.3.2's sequence that is not a config path: `blocking_rule=rule`
#: reads the loop variable, so the declaration marks the position and
#: :func:`render_call_sequence` substitutes the item it is repeating over.
EM_RULE: Final = "<em_blocking_rule>"


@dataclass(frozen=True)
class TrainCallSpec:
    """One row of S4.3.2's sequence: what to call, and which config field feeds each argument.

    `kwargs` maps a keyword name to a dotted path into the `Config` — never to a
    value. S4.3.2's whole point is that "every argument comes from the `training:`
    block", and a declaration holding literals would be a second source of tunables
    that no `config_hash` covers.
    """

    method_path: str
    kwargs: Mapping[str, str]
    #: A dotted path to a config list to repeat this call over, once per entry, in
    #: config order. ``None`` for the calls S4.3.2 issues exactly once.
    repeat_over: str | None = None


#: S4.3.2's fenced block, as data. The order is normative: the EM sessions refine an
#: m that `estimate_probability_two_random_records_match` and the u estimate have
#: already set priors for, so reordering these is a different model, not a different
#: spelling of one.
TRAIN_CALL_SEQUENCE: Final[tuple[TrainCallSpec, ...]] = (
    TrainCallSpec(
        "training.estimate_probability_two_random_records_match",
        {
            "deterministic_matching_rules": "training.deterministic_rules",
            "recall": "training.recall",
        },
    ),
    TrainCallSpec(
        "training.estimate_u_using_random_sampling",
        {"max_pairs": "training.u_max_pairs", "seed": "training.u_seed"},
    ),
    TrainCallSpec(
        "training.estimate_parameters_using_expectation_maximisation",
        {"blocking_rule": EM_RULE, "fix_u_probabilities": "training.em.fix_u_probabilities"},
        repeat_over="training.em_blocking_rules",
    ),
)

#: How the fitted settings are read back off a trained linker. Splink writes the file
#: only when it is given a path, and returns the dict either way, so this reads the
#: model without deciding where the artifact lands — that is ER-055's choice, and
#: S4.3.2 requires the object to be written before the registry row exists at all.
SETTINGS_READ_PATH: Final = "misc.save_model_to_json"

#: Keys Splink serializes into the settings document that are not part of the fitted
#: model. `linker_uid` is the whole list: Splink mints it as `ascii_uid(8)` in
#: `Linker.__init__` and uses it as the name prefix of the intermediates it caches, so
#: it differs between two runs that fitted identical m and u values — which is enough
#: on its own to make the artifact of a seeded training non-reproducible.
#:
#: Dropped rather than pinned to a constant. Splink mints a fresh one whenever a
#: settings document arrives without it, so nothing is lost on load; pinning it would
#: instead make the two linkers the S4.3.4 scorer builds on one connection agree on
#: their cache-table names, which is a collision rather than a determinism guarantee.
NON_FITTED_SETTINGS_KEYS: Final[tuple[str, ...]] = ("linker_uid",)


@dataclass(frozen=True)
class TrainCall:
    """One rendered call: a method path and the keyword arguments it receives."""

    method_path: str
    kwargs: Mapping[str, Any]


@dataclass(frozen=True)
class TrainResult:
    """What one training produced, short of anywhere to put it (ER-055).

    `settings` is the fitted model as Splink returns it; `training` is the S6 block
    the sequence was driven from, carried unmodified because S4.3.2 persists it
    verbatim into `model_registry.metrics` — and `metrics` holds that same object
    under its `training` key rather than a second rendering of it.
    """

    settings: dict[str, Any]
    training: dict[str, Any]
    metrics: dict[str, Any]
    tf_snapshot_id: str
    tf_rows: int
    #: The sequence this training actually issued, in order. Kept because the fitted
    #: values are otherwise the only evidence of how they were fitted.
    calls: tuple[TrainCall, ...] = field(default_factory=tuple)

    @property
    def settings_json(self) -> str:
        """`settings` as canonical JSON, the form S4.3.2 step 1 writes to S3.

        Keys are sorted for the same reason :func:`er.matching.model.settings_json`
        sorts them: the artifact is compared across processes, and mapping order is
        not part of what the settings mean.
        """
        return json.dumps(self.settings, sort_keys=True, indent=2)


class TrainLinker(Protocol):
    """A Splink `Linker`, narrowed to the two namespaces :func:`train_model` reaches.

    Both members are `Any` on purpose. The sequence is walked attribute by attribute
    from :data:`TRAIN_CALL_SEQUENCE`, so a narrower type would have to restate the
    three estimator signatures here — a fourth place for the call contract to live,
    when the point of the declaration is that there is one. What this Protocol pins
    is the namespacing: Splink 3's estimators hang off the bare linker, and a
    `training`-less object cannot satisfy it.
    """

    @property
    def training(self) -> Any:
        """`linker.training`: the S4.3.2 estimators."""

    @property
    def misc(self) -> Any:
        """`linker.misc`: where the fitted settings are read back from."""


def _resolve(cfg: Config, path: str) -> Any:
    """The value at a dotted path into `cfg`, e.g. ``training.em.fix_u_probabilities``."""
    return reduce(getattr, path.split("."), cfg)


def render_call_sequence(cfg: Config) -> tuple[TrainCall, ...]:
    """:data:`TRAIN_CALL_SEQUENCE` with every argument resolved against `cfg`.

    Args:
        cfg: the validated S6 document. Only `training:` is read, and S6.1
            normalization does not touch it, so a pre- or post-`normalize` document
            renders identically.

    Returns:
        The calls in S4.3.2 order, with one
        `estimate_parameters_using_expectation_maximisation` per
        `training.em_blocking_rules` entry, in config order.
    """
    calls: list[TrainCall] = []
    for spec in TRAIN_CALL_SEQUENCE:
        items: list[Any] = list(_resolve(cfg, spec.repeat_over)) if spec.repeat_over else [None]
        for item in items:
            calls.append(
                TrainCall(
                    spec.method_path,
                    {
                        name: item if source == EM_RULE else _resolve(cfg, source)
                        for name, source in spec.kwargs.items()
                    },
                )
            )
    return tuple(calls)


def build_training_linker(
    connection: duckdb.DuckDBPyConnection, cfg: Config, corpus_relation: str
) -> Linker:
    """The `Linker` `er train` fits, on the S4.0b connection and nothing else.

    The database API comes from :func:`er.matching.api.splink_api` — the repository's
    single construction site — so the primary database stays `:memory:`, the scratch
    schema is `splink_scratch`, and `threads` is pinned from `ER_DUCKDB_THREADS`.
    Constructing the API here instead would be a second chance to omit the output
    schema, and every `__splink__` intermediate EM materializes would then land in
    DuckLake as one snapshot per statement (M17).

    Args:
        connection: an open S4.0b connection with the lake attached by alias.
        cfg: the validated S6 document; `blocking:` and `comparisons:` become the
            settings through :func:`er.matching.model.build_settings`.
        corpus_relation: the **bare** name of a local relation holding the training
            corpus. Bare because Splink resolves an input table's columns through
            `information_schema.columns WHERE table_name = '<name>'`, which a
            schema-qualified name matches no row of; S4.0b's "materialized into local
            temp tables first" is the sanctioned way to get one.

    Raises:
        er.errors.StageFailure: the connection's default catalog is the lake.
        er.errors.ConfigError: the settings cannot be built from `cfg`.
    """
    return Linker(corpus_relation, settings=build_settings(cfg), db_api=splink_api(connection))


def _invoke(linker: TrainLinker, call: TrainCall) -> Any:
    """Issue one rendered call, walking the namespaced method path.

    `getattr` rather than a hard-coded `linker.training.<name>`: the path is declared
    in :data:`TRAIN_CALL_SEQUENCE`, and resolving it here is what makes the
    declaration the thing that runs rather than a comment beside the thing that runs.
    """
    target: Any = linker
    for attribute in call.method_path.split("."):
        target = getattr(target, attribute)
    return target(**call.kwargs)


def _fitted_settings(linker: TrainLinker) -> dict[str, Any]:
    """The fitted model, read off `linker` less :data:`NON_FITTED_SETTINGS_KEYS`.

    The removal is what makes the artifact a function of `(config, corpus)` alone, and
    therefore what makes the seed worth passing at all: two trainings that agreed on
    every m and u value would still write two different documents while Splink's
    per-linker cache uid rode along in one of the keys.
    """
    document = _invoke(linker, TrainCall(SETTINGS_READ_PATH, {}))
    if not isinstance(document, dict):
        raise StageFailure(
            f"{SETTINGS_READ_PATH} returned {type(document).__name__}, not the settings dict; "
            f"the fitted model is the artifact S4.3.2 step 1 writes under "
            f"storage.model_uri_prefix, and there is nothing to write without it"
        )
    return {key: value for key, value in document.items() if key not in NON_FITTED_SETTINGS_KEYS}


def train_model(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    corpus_relation: str,
    *,
    model_version: str,
    tf_snapshot_id: str | None = None,
    linker: TrainLinker | None = None,
) -> TrainResult:
    """Run the S4.3.2 sequence over `corpus_relation` and return what it fitted.

    The order of the whole stage is: freeze TF first, then fit. A crash between them
    leaves `tf_lookup` holding a key no `model_registry` row points at, which
    :func:`er.matching.tf.register_tf` and the registry both ignore; the other order
    would leave a fitted model whose TF snapshot does not exist, and D4's guarantee is
    exactly that a scored pair's TF is findable.

    Args:
        connection: an open S4.0b connection with the lake attached by alias.
        cfg: the validated S6 document. Every training argument comes from
            `training:` and from nothing else (S4.3.2, M26).
        corpus_relation: the bare name of the local relation to train over, per
            :func:`build_training_linker`.
        model_version: the zero-padded registry version the frozen TF rows belong to.
            ER-055 allocates it; this function only writes it down.
        tf_snapshot_id: the TF snapshot to freeze under. Minted here when omitted —
            `er train` is the first of the two callers D4 permits to mint one.
        linker: a linker to train instead of the one :func:`build_training_linker`
            would build. The seam the spy test uses; production leaves it unset.

    Returns:
        The fitted settings, the verbatim `training:` block, the metrics destined for
        `model_registry.metrics`, and the TF snapshot the corpus was frozen under.

    Raises:
        er.errors.ConfigError: a `tf: true` column is not a column of
            `int_std_records`.
        er.errors.StageFailure: the fitted settings could not be read back.
    """
    snapshot = new_tf_snapshot_id() if tf_snapshot_id is None else tf_snapshot_id
    tf_rows = materialize_tf_lookup(connection, cfg, model_version, snapshot)

    target = build_training_linker(connection, cfg, corpus_relation) if linker is None else linker
    calls = render_call_sequence(cfg)
    for call in calls:
        _invoke(target, call)

    settings = _fitted_settings(target)
    # `model_dump()` and not a re-derivation from the fields this module happens to
    # read: S4.3.2 persists the whole block verbatim, so a key the sequence never
    # touches must still survive into `model_registry.metrics`.
    training_block = cfg.training.model_dump()
    metrics: dict[str, Any] = {
        "training": training_block,
        "em_sessions": len(cfg.training.em_blocking_rules),
        # The one fitted scalar that is not per-comparison, and the one an operator
        # reads first when a model scores nothing.
        "probability_two_random_records_match": settings.get(
            "probability_two_random_records_match"
        ),
        "tf_snapshot_id": snapshot,
        "tf_columns": list(tf_columns(cfg)),
        "tf_rows": tf_rows,
    }
    return TrainResult(
        settings=settings,
        training=training_block,
        metrics=metrics,
        tf_snapshot_id=snapshot,
        tf_rows=tf_rows,
        calls=calls,
    )
