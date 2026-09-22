"""Incremental matching: two blocked Splink passes and one lake write (S4.3.4).

Both passes use the same frozen model and registered term-frequency tables.
`predict_between` scores the prior corpus against the batch; `predict_within`
deduplicates the batch itself. The prior corpus excludes every batch key, so each
pass has a distinct responsibility and a new pair cannot disappear silently when
one pass is missing. Both receive `review_low` explicitly as a probability.

Each pass keeps its own database API and linker state on the shared connection.
All inputs and intermediates stay in local scratch storage. The results are
unioned and sent to `full.merge_match_scores`, which canonicalizes, deduplicates
and persists them with one atomic MERGE. Existing scores remain cumulative.

Blocking rules come from the config's shared generator. `int_blocking_keys` is
never an input to scoring, and the incremental candidate-pair counter remains NULL
because the full-corpus count would not describe this batch.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import Any, Final

import duckdb
from splink import DuckDBAPI, Linker
from splink.internals.blocking import BlockingRule
from splink.internals.blocking_rule_creator import BlockingRuleCreator

from er.config.schema import Config
from er.entities.ids import IdFactory
from er.errors import ExitCode
from er.lake.bulk import insert_batches
from er.lake.columns import STD_RECORD_COLUMNS
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.api import assert_no_splink_relations_in_lake, splink_api
from er.matching.evidence import build_evidence
from er.matching.full import (
    MATCH_SCORES_RELATION,
    RETAIN_INTERMEDIATE_KEY,
    ScoredPair,
    merge_match_scores,
    prediction_columns,
    review_scored_pairs,
)
from er.matching.model import LINK_TYPE, UNIQUE_ID_COLUMN, blocking_rules_from_config
from er.matching.tf import (
    STD_RECORDS_RELATION,
    assert_tf_lookup_complete,
    register_tf,
    tf_columns,
)
from er.obs.profiling import profiled
from er.obs.runctx import StageRun

# `MODE_INCREMENTAL` is re-exported below rather than spelled again: `runs.mode`, S4.0's
# `--mode` value and this stage's `mode` counter are one string under three names, and
# :mod:`er.versions` is where the two modes are declared.
from er.versions import MODE_INCREMENTAL

__all__ = [
    "BATCH_KEYS_RELATION",
    "BATCH_RELATION",
    "EVIDENCE_COLUMN",
    "LINK_TYPE_KEY",
    "MODE_INCREMENTAL",
    "PRIOR_CORPUS_RELATION",
    "UNION_RELATION",
    "IncrementalScoreResult",
    "pass1_new_vs_corpus",
    "pass2_new_vs_new",
    "score_incremental",
    "unscored_record_keys",
]

#: The four relations this stage materializes, all of them bare and local. Bare for the
#: reason `er.matching.full` copies its corpus — Splink resolves an input table's
#: columns through `information_schema.columns WHERE table_name = '<name>'`, which a
#: lake-qualified name matches no row of — and local because S4.0b permits one write to
#: the lake per scoring stage, which is the `MERGE INTO` at the end and nothing else.
BATCH_KEYS_RELATION: Final = "er_match_batch_keys"
BATCH_RELATION: Final = "er_match_batch"
PRIOR_CORPUS_RELATION: Final = "er_match_prior_corpus"
UNION_RELATION: Final = "er_match_incremental_pairs"

#: The column the unioned relation carries its already-rendered `evidence JSON` in. The
#: two passes need not emit the same prediction columns, so each is projected through
#: its **own** :func:`~er.matching.evidence.build_evidence` expression before the union;
#: by the time the merge reads the result the payload is a column, and the "expression"
#: it is handed is that column's name.
EVIDENCE_COLUMN: Final = "evidence"

#: The settings key S4.3.4 switches for pass 2. The frozen artifact already carries
#: :data:`~er.matching.model.LINK_TYPE` (v1 resolves one corpus against itself), so
#: setting it is a no-op today — and is written anyway, because the spec states pass 2
#: as "the frozen settings with `link_type='dedupe_only'`" and a model artifact that
#: ever carried a different value must not silently change what pass 2 means.
LINK_TYPE_KEY: Final = "link_type"

_STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.{STD_RECORDS_RELATION}"
_MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.{MATCH_SCORES_RELATION}"

_LEFT: Final = f"{UNIQUE_ID_COLUMN}_l"
_RIGHT: Final = f"{UNIQUE_ID_COLUMN}_r"

#: Every record this key has already scored, as `(record_key, ingest_batch_id)`.
#:
#: "Already scored" is `content_hash`-exact, because INV-SCORE (S4.3.3) makes the two
#: endpoint hashes part of the scoring key: a record whose standardized attributes
#: changed is a *different* scoring problem, and the row carrying its old hash is not a
#: score of the record as it now stands.
_SCORED_RECORDS_SQL: Final = f"""
SELECT rec.{UNIQUE_ID_COLUMN}, rec.ingest_batch_id
  FROM {_STD_RECORDS} AS rec
  JOIN {_MATCH_SCORES} AS score
    ON (score.rec_a_key = rec.{UNIQUE_ID_COLUMN} AND score.rec_a_content_hash = rec.content_hash)
    OR (score.rec_b_key = rec.{UNIQUE_ID_COLUMN} AND score.rec_b_content_hash = rec.content_hash)
 WHERE score.model_version = ? AND score.tf_snapshot_id = ?
"""

#: S4.0's "no unscored records", as a query. The second clause is the load-bearing one;
#: :func:`unscored_record_keys` explains why it is there.
_UNSCORED_KEYS_SQL: Final = f"""
WITH scored AS ({_SCORED_RECORDS_SQL})
SELECT rec.{UNIQUE_ID_COLUMN}
  FROM {_STD_RECORDS} AS rec
 WHERE rec.{UNIQUE_ID_COLUMN} NOT IN (SELECT {UNIQUE_ID_COLUMN} FROM scored)
   AND rec.ingest_batch_id NOT IN (SELECT ingest_batch_id FROM scored)
 ORDER BY rec.{UNIQUE_ID_COLUMN}
"""

_BATCH_KEYS_DDL: Final = (
    f"CREATE OR REPLACE TABLE {BATCH_KEYS_RELATION} ({UNIQUE_ID_COLUMN} VARCHAR NOT NULL)"
)
_BATCH_KEYS_INSERT: Final = f"INSERT INTO {BATCH_KEYS_RELATION} SELECT unnest(?::VARCHAR[])"

#: The batch and the prior corpus: one projection of `int_std_records`, split by the
#: key relation. `NOT IN` over a relation of non-NULL keys, which is what the `NOT NULL`
#: in the DDL above is for — a single NULL would make `NOT IN` empty and silently score
#: the batch against nothing.
_BATCH_SQL: Final = (
    f"CREATE OR REPLACE TABLE {BATCH_RELATION} AS "
    f"SELECT {', '.join(STD_RECORD_COLUMNS)} FROM {_STD_RECORDS} "
    f"WHERE {UNIQUE_ID_COLUMN} IN (SELECT {UNIQUE_ID_COLUMN} FROM {BATCH_KEYS_RELATION})"
)
_PRIOR_CORPUS_SQL: Final = (
    f"CREATE OR REPLACE TABLE {PRIOR_CORPUS_RELATION} AS "
    f"SELECT {', '.join(STD_RECORD_COLUMNS)} FROM {_STD_RECORDS} "
    f"WHERE {UNIQUE_ID_COLUMN} NOT IN (SELECT {UNIQUE_ID_COLUMN} FROM {BATCH_KEYS_RELATION})"
)


def _count(connection: duckdb.DuckDBPyConnection, relation: str) -> int:
    row = connection.execute(f"SELECT count(*) FROM {relation}").fetchone()
    assert row is not None, "count(*) returns a row"
    return int(row[0])


def unscored_record_keys(
    connection: duckdb.DuckDBPyConnection,
    *,
    model_version: str,
    tf_snapshot_id: str,
) -> tuple[str, ...]:
    """The records this `(model_version, tf_snapshot_id)` has not scored yet (S4.0).

    This is the set S4.0's exit ``10`` is about — "incremental mode with no unscored
    records" — and therefore also the batch the two passes run over. The spec defines
    the exit code and not the set, so the definition is here, and it is two clauses:

    1. **No `match_scores` row at this key carries the record with its CURRENT
       `content_hash`.** Content-exact because of INV-SCORE (S4.3.3): a record whose
       standardized attributes changed is a different scoring problem, and a row holding
       its previous hash is not a score of the record as it now stands. This is the
       clause that makes newly ingested and re-standardized records the batch.

    2. **No record of the same `ingest_batch_id` is scored either.** Clause 1 alone
       would report a record that was scored *and found no partner* as unscored
       forever — `base_10`'s `crm:C008` is a persona with one source record and no pair
       at all — so every later run would re-score it and S4.0's ``10`` would be
       unreachable. Batch granularity is how `er standardize` answers the same question
       (`dbt/models/staging/stg_*.sql` uses the not-in-distinct-batch predicate and
       explicitly *not* a `>` watermark on `ingested_at`, so that a replayed batch
       contributes nothing), and a batch that produced any scored endpoint has been
       through a scoring run.

    The residual case the pair leaves open is a batch whose every record scored no pair
    at all: it stays unscored and is offered to the next run again. That is the safe
    direction — re-scoring is idempotent on the logical key (S4.3.4) — where the
    opposite, a record silently never offered to a scorer, is an entity that never forms.

    An empty `match_scores` makes every record unscored, so a lake with no prior run
    scores its whole corpus as one batch: pass 1 has no prior corpus and pass 2 is a
    corpus-wide `dedupe_only` predict, which is `--mode full` reached by another road.

    Args:
        connection: an open S4.0b connection with the lake attached by alias. Nothing
            is written; this reads `int_std_records` and `match_scores`.
        model_version: the registry version the run scores at.
        tf_snapshot_id: the frozen TF snapshot the run registers.

    Returns:
        The `record_key`s, sorted — the order the batch relation is built in and the
        order a `rows_in` count is taken over.
    """
    rows = connection.execute(_UNSCORED_KEYS_SQL, [model_version, tf_snapshot_id]).fetchall()
    return tuple(str(record_key) for (record_key,) in rows)


@profiled("match.prepare_batch", "records")
def _materialize_batch(connection: duckdb.DuckDBPyConnection, keys: Sequence[str]) -> int:
    """Split `int_std_records` into the batch and the prior corpus; return the latter's size.

    Called **after** :func:`~er.matching.api.splink_api`, for the reason
    :func:`er.matching.full._materialize_corpus` gives: the API's constructor issues
    ``SET schema``, so an unqualified ``CREATE OR REPLACE TABLE`` lands in the scratch
    schema Splink resolves its own relations in. Materializing earlier would leave one
    copy in `main` and one in the scratch schema, and Splink resolves an input table
    through `information_schema.columns WHERE table_name = '<name>'`, which two copies
    match twice.
    """
    connection.execute(_BATCH_KEYS_DDL)
    insert_batches(
        connection,
        _BATCH_KEYS_INSERT,
        ((key,) for key in keys),
        columns=1,
    )
    connection.execute(_BATCH_SQL)
    connection.execute(_PRIOR_CORPUS_SQL)
    return _count(connection, PRIOR_CORPUS_RELATION)


def _pass_settings(settings: Mapping[str, Any], **overrides: Any) -> dict[str, Any]:
    """The frozen settings with the evidence columns retained, plus ``overrides``.

    A copy, so the caller's document — the artifact `model_registry` points at — is not
    mutated by having been scored with. `retain_intermediate_calculation_columns` is
    forced on for both passes for the reason `er.matching.full`'s module docstring
    gives: the flag decides which columns the prediction *emits*, not what it computes,
    and S4.3.5 requires the `gamma_*` vector and the per-comparison Bayes factors to be
    retained rather than projected away.
    """
    return {**settings, RETAIN_INTERMEDIATE_KEY: True, **overrides}


def _pair_select(connection: duckdb.DuckDBPyConnection, cfg: Config, relation: str) -> str:
    """One pass's prediction, projected onto the four columns the union carries.

    The evidence expression is built from *this* relation's columns
    (:func:`~er.matching.full.prediction_columns`), not from the config alone: which
    columns a prediction emits is a property of the call that produced it, and
    `predict_between` and `predict_within` are two different calls. Rendering the
    payload here rather than after the union is what lets the two passes disagree about
    their column sets without the union having to reconcile them.
    """
    evidence = build_evidence(cfg, prediction_columns(connection, relation))
    return (
        f"SELECT {_LEFT}, {_RIGHT}, match_probability, {evidence} AS {EVIDENCE_COLUMN} "
        f"FROM {relation}"
    )


@profiled("match.new_vs_corpus", "pairs")
def pass1_new_vs_corpus(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    api: DuckDBAPI,
    settings: Mapping[str, Any],
    *,
    model_version: str,
    tf_snapshot_id: str,
) -> str | None:
    """Pass 1: score blocked pairs between the prior corpus and new records.

    Both inputs are registered with this pass's API before constructing the linker.
    Frozen TF is registered for every configured column. The same blocking-rule
    generator feeds dbt and Splink, and review_low is passed as a probability.
    Return a SELECT carrying canonical-writer inputs, or None for an empty corpus."""
    if _count(connection, PRIOR_CORPUS_RELATION) == 0:
        return None
    _, generated = blocking_rules_from_config(cfg)
    # A copy widened to Splink's own parameter type. `list` is invariant, so the
    # generator's `list[BlockingRuleCreator]` is not assignable to a list of the union
    # Splink accepts; `er.matching.model.build_settings` widens the same list the same
    # way for the same reason.
    rules: list[BlockingRuleCreator | BlockingRule | dict[str, Any] | str] = list(generated)
    prior = api.register(PRIOR_CORPUS_RELATION)
    batch = api.register(BATCH_RELATION)
    linker = Linker(prior, settings=_pass_settings(settings))
    register_tf(linker, connection, cfg, model_version, tf_snapshot_id, db_api=api)
    predictions = linker.inference.predict_between(
        left=prior,
        right=batch,
        blocking_rules_to_generate_predictions=rules,
        threshold_match_probability=cfg.thresholds.review_low,
    )
    return _pair_select(connection, cfg, str(predictions.physical_name))


@profiled("match.new_vs_new", "pairs")
def pass2_new_vs_new(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    api: DuckDBAPI,
    settings: Mapping[str, Any],
    *,
    model_version: str,
    tf_snapshot_id: str,
) -> str | None:
    """Pass 2: deduplicate the new batch with the same frozen model and TF.

    predict_between never compares two records on the same side, so this pass is
    required for two records arriving together to form a new entity. The batch-only
    linker uses dedupe_only and predict_within with an explicit review_low
    probability. Return None when the batch contains fewer than two records."""
    if _count(connection, BATCH_RELATION) < 2:
        return None
    batch = api.register(BATCH_RELATION)
    linker = Linker(
        batch,
        settings=_pass_settings(settings, **{LINK_TYPE_KEY: LINK_TYPE}),
    )
    register_tf(linker, connection, cfg, model_version, tf_snapshot_id, db_api=api)
    predictions = linker.inference.predict_within(
        batch, threshold_match_probability=cfg.thresholds.review_low
    )
    return _pair_select(connection, cfg, str(predictions.physical_name))


@dataclass(frozen=True)
class IncrementalScoreResult:
    """What one `er match --mode incremental` scored, in the terms S4.0 prints it in.

    The same shape :class:`~er.matching.full.FullMatchResult` has, because S4.0 gives
    `er match` one stdout line and one counter list whichever mode ran, plus the one
    field only this mode has: `unscored_records`, which is `rows_in` and is what decides
    :attr:`exit_code`.
    """

    mode: str
    model_version: str
    tf_snapshot_id: str
    candidate_pairs: int | None
    unscored_records: int
    pairs_scored: int
    pairs_above_auto_merge: int
    pairs_in_gray_band: int
    review_queue_added: int
    review_queue_refreshed: int

    @property
    def exit_code(self) -> int:
        """S4.0: ``10`` for "incremental mode with no unscored records", else ``0``.

        The condition is the *input* being empty, not the output: a batch that scored no
        pair above `review_low` did work and did have something to do — the records were
        offered to both passes and matched nothing — and reporting that as "nothing to
        do" would hide a run whose blocking or model had stopped producing candidates.
        """
        if self.unscored_records == 0:
            return int(ExitCode.NOTHING_TO_DO)
        return int(ExitCode.SUCCESS)

    def manifest(self) -> dict[str, object]:
        """The `--json` stdout object: S4.0's seven fields for `er match`."""
        return {
            "mode": self.mode,
            "model_version": self.model_version,
            "tf_snapshot_id": self.tf_snapshot_id,
            "candidate_pairs": self.candidate_pairs,
            "pairs_scored": self.pairs_scored,
            "pairs_above_auto_merge": self.pairs_above_auto_merge,
            "review_queue_added": self.review_queue_added,
        }

    def stdout_line(self) -> str:
        """:meth:`manifest` as the human summary S4.0's stdout column describes."""
        return ", ".join(f"{name}={_render(value)}" for name, value in self.manifest().items())

    def record(self, stage_run: StageRun) -> None:
        """Write this stage's counters onto its `run_stages` row (S4.3.5, S5.2).

        Every name S4.3.5's counter paragraph lists, plus S5.2's `rows_in`/`rows_out`:
        records offered to the passes in, pairs persisted out. `candidate_pairs` is set
        to ``None`` deliberately — S4.3.4 assigns `candidate_pair_count` to
        `int_blocking_keys`, which is not an input to this stage and whose corpus-wide
        value would describe the full path — and setting it is what puts the typed
        column and the payload entry there as NULL rather than absent (S5.2).
        """
        stage_run.model_version = self.model_version
        stage_run.tf_snapshot_id = self.tf_snapshot_id
        counters = stage_run.counters
        counters.set("rows_in", self.unscored_records)
        counters.set("rows_out", self.pairs_scored)
        counters.set("mode", self.mode)
        counters.set("model_version", self.model_version)
        counters.set("tf_snapshot_id", self.tf_snapshot_id)
        counters.set("candidate_pairs", self.candidate_pairs)
        counters.set("pairs_scored", self.pairs_scored)
        counters.set("pairs_above_auto_merge", self.pairs_above_auto_merge)
        counters.set("pairs_in_gray_band", self.pairs_in_gray_band)
        counters.set("review_queue_added", self.review_queue_added)
        counters.set("review_queue_refreshed", self.review_queue_refreshed)


def _render(value: object) -> str:
    """A stdout field, with NULL as `\\N` — the spelling `er review`'s line uses."""
    return "\\N" if value is None else str(value)


def _nothing_to_do(model_version: str, tf_snapshot_id: str) -> IncrementalScoreResult:
    """The S4.0 exit-``10`` result: no unscored records, so no pass ran and nothing wrote."""
    return IncrementalScoreResult(
        mode=MODE_INCREMENTAL,
        model_version=model_version,
        tf_snapshot_id=tf_snapshot_id,
        candidate_pairs=None,
        unscored_records=0,
        pairs_scored=0,
        pairs_above_auto_merge=0,
        pairs_in_gray_band=0,
        review_queue_added=0,
        review_queue_refreshed=0,
    )


@profiled("match.incremental", "pairs")
def score_incremental(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    run_ctx: StageRun,
    *,
    model_version: str,
    tf_snapshot_id: str,
    settings: Mapping[str, Any],
    record_keys: Sequence[str] | None = None,
    scored_at: datetime | None = None,
    id_factory: IdFactory | None = None,
) -> IncrementalScoreResult:
    """Score a batch as two unioned Splink passes and persist it in one statement (S4.3.4).

    The order below is the section's, and every step of it is a decision:

    1. **Decide the batch before anything is built.** :func:`unscored_record_keys` is a
       read of the lake, and an empty answer is S4.0's ``10``: no linker, no
       registration, no write, and a `run_stages` row that says the stage succeeded with
       nothing to do.
    2. **Build the API, then split the corpus.** The API comes from
       :func:`~er.matching.api.splink_api` — the repository's one construction site —
       and the split comes after it, so both relations land in the in-memory scratch
       schema (:func:`_materialize_batch`).
    3. **Run both passes**, each registering the frozen TF of the same key (D4, S4.3.3):
       pass 1 against the prior corpus and pass 2 within the batch, both at the review
       probability threshold. Either may contribute nothing.
    4. **Union, then merge once.** The union is `UNION ALL` into a local relation; the
       self-pair drop, the canonicalisation to `rec_a_key < rec_b_key`, the `DISTINCT`
       and the endpoint-hash join all happen inside
       :func:`~er.matching.full.merge_match_scores`, which is also full mode's writer —
       so a pair both passes found is one row, and the relation stays cumulative.
    5. **Classify the result in Python** through :mod:`er.matching.thresholds` and
       upsert the gray band to `review_queue` (S4.3.5). Nothing in the band is an edge:
       the clustering threshold **is** `auto_merge`.
    6. **Assert M17** before returning, so a leak is attributed to this stage rather
       than to whatever ran next.

    Args:
        connection: an open S4.0b connection with the lake attached by alias.
        cfg: the validated S6 document; `blocking:`, `comparisons:` and `thresholds:`.
        run_ctx: this stage's `run_stages` row — the source of `run_id` and the
            destination of every counter S4.3.5 lists.
        model_version: the registry version being scored at, written onto every row.
        tf_snapshot_id: the frozen TF snapshot to register, written onto every row.
        settings: the settings document `model_version` was registered with. Passed in
            rather than fetched, because the object store is the CLI's to construct and
            a scenario test loads the committed fixture model instead (S4.3.2 item 6).
        record_keys: the batch, when the caller already knows it.
            :func:`unscored_record_keys` decides it otherwise, which is what the CLI
            does. An explicit batch is how a caller re-scores a set the lake would now
            call scored — the idempotence claim of S4.3.4 is about the logical key, not
            about the batch being recomputed.
        scored_at: the stamp every row this run writes carries; now, in UTC, when
            omitted.
        id_factory: the source of minted `review_id`s (S4.5.4, D10).

    Returns:
        The S4.0 stdout fields and the S4.3.5 counters, already recorded on ``run_ctx``.
        :attr:`IncrementalScoreResult.exit_code` is ``10`` when the batch was empty.

    Raises:
        er.errors.StageFailure: a prediction carries no evidence columns, or a persisted
            pair is not canonically ordered. Both are S4.0 exit ``1``.
        er.matching.tf.TfLookupIncomplete: the preflight found one or more `tf: true`
            columns with no frozen rows for this key. Raised before the batch is
            decided and before anything is built or written, and it names every
            missing column (exit ``3``).
        er.matching.tf.MissingTfLookupError: no frozen TF rows for this key (exit ``3``).
    """
    # D4 preflight, ahead of even the batch decision. An incomplete frozen snapshot is
    # a precondition failure of the STAGE, not a property of the batch: reporting
    # S4.0's `10` ("nothing to do") for a lake whose `tf_lookup` is missing columns
    # would hide a misconfigured model until the next delivery happened to be
    # non-empty, and that delivery is the one that would score wrongly.
    assert_tf_lookup_complete(connection, model_version, tf_snapshot_id, tf_columns(cfg))

    thresholds = cfg.thresholds
    keys = (
        unscored_record_keys(connection, model_version=model_version, tf_snapshot_id=tf_snapshot_id)
        if record_keys is None
        else tuple(record_keys)
    )
    if not keys:
        result = _nothing_to_do(model_version, tf_snapshot_id)
        result.record(run_ctx)
        return result

    # Keep input registration and linker state scoped to each pass, while using
    # the same connection and frozen model. The first API also selects the local
    # scratch schema before the corpus is split into its two physical tables.
    corpus_api = splink_api(connection)
    _materialize_batch(connection, keys)
    batch_api = splink_api(connection)

    # Module-global lookups, so that AC2's falsification — replace pass 2 and the
    # new-vs-new pair must disappear — is expressible without reaching into the union.
    selects = [
        select
        for select in (
            pass1_new_vs_corpus(
                connection,
                cfg,
                corpus_api,
                settings,
                model_version=model_version,
                tf_snapshot_id=tf_snapshot_id,
            ),
            pass2_new_vs_new(
                connection,
                cfg,
                batch_api,
                settings,
                model_version=model_version,
                tf_snapshot_id=tf_snapshot_id,
            ),
        )
        if select is not None
    ]

    scored: Iterable[ScoredPair] = ()
    if selects:
        # `UNION ALL`, not `UNION`: the deduplication a pair found by both passes needs
        # is the merge source's `DISTINCT ON (rec_a_key, rec_b_key)`, which also
        # canonicalises the pair first — a `UNION` here would collapse only the rows
        # that already agreed on orientation and hide half the work from the one place
        # that does it properly (S4.3.4).
        connection.execute(
            f"CREATE OR REPLACE TABLE {UNION_RELATION} AS {' UNION ALL '.join(selects)}"
        )
        scored = merge_match_scores(
            connection,
            UNION_RELATION,
            EVIDENCE_COLUMN,
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            run_id=run_ctx.run_id,
            scored_at=scored_at,
        )

    summary = review_scored_pairs(
        connection,
        scored,
        thresholds,
        run_id=run_ctx.run_id,
        id_factory=id_factory,
    )

    assert_no_splink_relations_in_lake(connection)

    result = IncrementalScoreResult(
        mode=MODE_INCREMENTAL,
        model_version=model_version,
        tf_snapshot_id=tf_snapshot_id,
        candidate_pairs=None,
        unscored_records=len(keys),
        pairs_scored=summary.pairs_scored,
        pairs_above_auto_merge=summary.pairs_above_auto_merge,
        pairs_in_gray_band=summary.pairs_in_gray_band,
        review_queue_added=summary.review_queue_added,
        review_queue_refreshed=summary.review_queue_refreshed,
    )
    result.record(run_ctx)
    return result
