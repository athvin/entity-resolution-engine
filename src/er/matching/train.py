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

:func:`train_model` is the sequence and nothing else. :func:`run_train_stage` is the
`er train` stage built on top of it: it captures the corpus snapshot the model is
fitted against, applies the S4 `--if-changed` idempotency key, allocates the version
the TF rows are frozen under, and hands the fitted settings to
:mod:`er.lake.model_registry`, which owns the artifact upload, the row and the
active/superseded pointer. What it does own beyond the sequence is the `tf_lookup`
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
from er.errors import ExitCode, StageFailure
from er.lake.columns import STD_RECORD_COLUMNS
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import (
    ModelRow,
    ObjectWriter,
    allocate_model_version,
    corpus_snapshot,
    find_active_model,
    register_model,
)
from er.matching.api import splink_api
from er.matching.model import build_settings
from er.matching.tf import (
    STD_RECORDS_RELATION,
    materialize_tf_lookup,
    new_tf_snapshot_id,
    tf_columns,
    tf_tables_path,
)
from er.obs.runctx import StageRun

__all__ = [
    "EM_RULE",
    "FITTED_METRICS_KEY",
    "NON_FITTED_SETTINGS_KEYS",
    "SETTINGS_READ_PATH",
    "TRAIN_CALL_SEQUENCE",
    "TRAIN_CORPUS_RELATION",
    "TrainCall",
    "TrainCallSpec",
    "TrainLinker",
    "TrainResult",
    "TrainStageResult",
    "build_training_linker",
    "fitted_m_u",
    "render_call_sequence",
    "run_train_stage",
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


#: The bare local relation `er train` copies the corpus into. Bare and local for
#: :func:`build_training_linker`'s reason, and named once here because the stage
#: creates it and Splink resolves it.
TRAIN_CORPUS_RELATION: Final = "er_train_corpus"

#: Where the fitted m and u values land in `model_registry.metrics`. S4.3.2 requires
#: the metrics payload to carry them alongside the verbatim `training:` block; the
#: block alone would record how the model was fitted but not what was fitted.
FITTED_METRICS_KEY: Final = "fitted_m_u"

_CORPUS_SQL: Final = (
    f"CREATE OR REPLACE TABLE {TRAIN_CORPUS_RELATION} AS "
    f"SELECT {', '.join(STD_RECORD_COLUMNS)} "
    f"FROM {SCHEMA_QUALIFIER}.{STD_RECORDS_RELATION}"
)

#: The columns are projected by name and never `SELECT *`: S5.1 makes explicit
#: projection the rule for reading a relation that may have grown a column, and the
#: list is S5's own (:data:`~er.lake.columns.STD_RECORD_COLUMNS`).
_CORPUS_COUNT_SQL: Final = f"SELECT count(*) FROM {TRAIN_CORPUS_RELATION}"


def _materialize_corpus(connection: duckdb.DuckDBPyConnection) -> int:
    """Copy the corpus into a bare local relation and return how many rows it holds.

    Local because Splink resolves an input table through `information_schema.columns
    WHERE table_name = '<name>'`, which a lake-qualified name matches no row of;
    S4.0b's "materialized into local temp tables first" is the sanctioned way to get
    one (see :func:`build_training_linker`).

    Raises:
        er.errors.StageFailure: the corpus relation does not exist, or holds no rows.
            Both are `er standardize` not having run, and both are worth naming here:
            Splink's own failure over an absent or empty frame arrives much later and
            says nothing about the pipeline step that was skipped.
    """
    try:
        connection.execute(_CORPUS_SQL)
    except duckdb.Error as exc:
        raise StageFailure(
            f"{SCHEMA_QUALIFIER}.{STD_RECORDS_RELATION} cannot be read: {exc}. "
            f"`er train` fits over the standardized corpus, so `er standardize` runs "
            f"first (S4.0, S4.3.2)"
        ) from exc
    counted = connection.execute(_CORPUS_COUNT_SQL).fetchone()
    rows = 0 if counted is None else int(counted[0])
    if rows == 0:
        raise StageFailure(
            f"{SCHEMA_QUALIFIER}.{STD_RECORDS_RELATION} holds no rows; there is nothing "
            f"to fit an EM model over (S4.3.2)"
        )
    return rows


def fitted_m_u(settings: Mapping[str, Any]) -> list[dict[str, Any]]:
    """The fitted m and u of every comparison level, in settings order.

    A list rather than a mapping keyed by level label: Splink's labels are display
    strings and two levels of one comparison may share one, so a mapping would drop a
    fitted value and no reader would notice.
    """
    fitted: list[dict[str, Any]] = []
    comparisons = settings.get("comparisons", ())
    if not isinstance(comparisons, list):
        return fitted
    for comparison in comparisons:
        name = comparison.get("output_column_name")
        for level in comparison.get("comparison_levels", ()):
            fitted.append(
                {
                    "comparison": name,
                    "level": level.get("label_for_charts"),
                    "m_probability": level.get("m_probability"),
                    "u_probability": level.get("u_probability"),
                }
            )
    return fitted


@dataclass(frozen=True)
class TrainStageResult:
    """What one `er train` invocation produced, in the terms S4.0 prints it in.

    ``trained`` distinguishes the two exits S4.0 gives the command: a run that fitted
    and registered a model (`0`), and a `--if-changed` run that found the active row
    already carrying this `(config_hash, corpus_snapshot)` and allocated nothing
    (`10`). Both report the same five fields, because the second one is describing the
    model that made the training unnecessary.
    """

    model_version: str
    params_path: str
    tf_snapshot_id: str
    corpus_snapshot: int
    metrics: Mapping[str, Any]
    trained: bool
    rows_in: int

    @property
    def exit_code(self) -> int:
        """S4.0: ``0`` for a training run, ``10`` for a `--if-changed` no-op."""
        return int(ExitCode.SUCCESS if self.trained else ExitCode.NOTHING_TO_DO)

    def manifest(self) -> dict[str, object]:
        """The `--json` stdout object: S4.0's five fields for `er train`."""
        return {
            "model_version": self.model_version,
            "params_path": self.params_path,
            "tf_snapshot_id": self.tf_snapshot_id,
            "corpus_snapshot": self.corpus_snapshot,
            "metrics": dict(self.metrics),
        }

    def stdout_line(self) -> str:
        """The human stdout line, in S4.0's column order.

        `metrics` is rendered as compact JSON rather than expanded: it is one of the
        five fields S4.0 names, and a multi-line rendering would stop the line being
        one line.
        """
        metrics = json.dumps(dict(self.metrics), sort_keys=True, separators=(",", ":"))
        return (
            f"model_version={self.model_version}, params_path={self.params_path}, "
            f"tf_snapshot_id={self.tf_snapshot_id}, "
            f"corpus_snapshot={self.corpus_snapshot}, metrics={metrics}"
        )

    def record(self, stage_run: StageRun) -> None:
        """Write this run's counters onto its `run_stages` row (S5.2).

        S4 declares no per-stage counter list for `train`, so every name here is
        free-form and lands in the `counters` JSON payload — except `rows_in`, which
        S5.2 promotes to a typed column and :meth:`StageCounters.set` routes there.
        The three names S4.3.2 makes the stage's own identity — the version, the TF
        snapshot and the corpus it was fitted against — are also copied onto the
        :class:`~er.obs.runctx.StageRun` itself, because the S5.2 stderr record carries
        `model_version` and `tf_snapshot_id` as top-level keys.
        """
        stage_run.model_version = self.model_version
        stage_run.tf_snapshot_id = self.tf_snapshot_id
        stage_run.counters.set("rows_in", self.rows_in)
        stage_run.counters.set("model_version", self.model_version)
        stage_run.counters.set("tf_snapshot_id", self.tf_snapshot_id)
        stage_run.counters.set("corpus_snapshot", self.corpus_snapshot)
        stage_run.counters.set("trained", self.trained)


def _unchanged(active: ModelRow, config_hash: str, snapshot: int) -> bool:
    """Whether the active row already carries this `(config_hash, corpus_snapshot)`.

    S4's idempotency key for `train`, and the whole of what `--if-changed` tests. The
    ACTIVE row and not any row: a superseded model carrying the same key was trained
    and then replaced, and re-training is exactly what the operator wants after that.
    """
    return active.config_hash == config_hash and active.corpus_snapshot == snapshot


def run_train_stage(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    store: ObjectWriter,
    *,
    run_id: str,
    config_hash: str,
    if_changed: bool = False,
    stage_run: StageRun | None = None,
) -> TrainStageResult:
    """The `er train` stage: fit a model, register it, and activate it (S4.3.2, S4.0).

    The order is fixed by S4.3.2 and by S4's idempotency table, and every step of it is
    a decision some other order would get wrong:

    1. **Capture `corpus_snapshot` first.** It is the version the model is fitted
       against, so it has to be read before anything this stage does can advance it.
    2. **Apply `--if-changed` second**, against the ACTIVE row's
       `(config_hash, corpus_snapshot)`. Before the allocation, because "allocates no
       new `model_version`" is the whole content of the S4 row.
    3. **Allocate, then freeze TF, then fit.** `tf_lookup` is keyed by
       `(model_version, tf_snapshot_id)`, so the version has to exist before the rows
       do; :func:`register_model` re-derives the same allocation inside its transaction
       and refuses if it moved.
    4. **Register last.** The artifact is uploaded before the row is written, and both
       happen only once there is a fitted model to point at.

    Args:
        connection: an open S4.0b connection with the lake attached by alias.
        cfg: the validated S6 document.
        store: where the artifact goes — an
            :class:`~er.lake.objectstore.ObjectStore` in production.
        run_id: this invocation's run id (S5.2).
        config_hash: this invocation's `config_hash`; half of the S4 idempotency key
            and a column of the registry row.
        if_changed: the S4.0 `--if-changed` flag.
        stage_run: the `run_stages` row to record counters on, when there is one.

    Returns:
        The five fields S4.0 prints, and the exit code they imply.

    Raises:
        er.errors.ConfigError: a TF column is not a column of `int_std_records`.
        er.errors.StageFailure: the fitted settings could not be read back, or the
            version allocation moved under the stage.
    """
    snapshot = corpus_snapshot(connection, STD_RECORDS_RELATION)
    active = find_active_model(connection)
    if if_changed and active is not None and _unchanged(active, config_hash, snapshot):
        skipped = TrainStageResult(
            model_version=active.model_version,
            params_path=active.params_path,
            tf_snapshot_id=active.tf_snapshot_id,
            corpus_snapshot=active.corpus_snapshot,
            metrics=active.metrics,
            trained=False,
            rows_in=0,
        )
        if stage_run is not None:
            skipped.record(stage_run)
        return skipped

    model_version = allocate_model_version(connection)
    rows_in = _materialize_corpus(connection)
    result = train_model(connection, cfg, TRAIN_CORPUS_RELATION, model_version=model_version)
    metrics: dict[str, Any] = {**result.metrics, FITTED_METRICS_KEY: fitted_m_u(result.settings)}

    row = register_model(
        connection,
        store,
        model_version=model_version,
        model_uri_prefix=cfg.storage.model_uri_prefix,
        settings_json=result.settings_json,
        metrics=metrics,
        corpus_snapshot=snapshot,
        tf_snapshot_id=result.tf_snapshot_id,
        tf_tables_path=tf_tables_path(model_version, result.tf_snapshot_id),
        config_hash=config_hash,
        run_id=run_id,
    )
    stage_result = TrainStageResult(
        model_version=row.model_version,
        params_path=row.params_path,
        tf_snapshot_id=row.tf_snapshot_id,
        corpus_snapshot=row.corpus_snapshot,
        metrics=row.metrics,
        trained=True,
        rows_in=rows_in,
    )
    if stage_run is not None:
        stage_result.record(stage_run)
    return stage_result
