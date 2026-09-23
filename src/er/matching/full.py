"""Full scoring with frozen parameters/TF and config-generated prediction rules.

Predictions and evidence remain SQL relations. One MERGE writes the final scores;
full mode retires absent active pairs in the same transaction. A correction also
activates its journaled TF snapshot in that transaction. Historical rows remain
queryable, and review evidence is classified in DuckDB after score publication.
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Iterator, Mapping
from contextlib import contextmanager, suppress
from dataclasses import dataclass
from datetime import UTC, datetime
from itertools import batched
from types import MappingProxyType
from typing import Any, Final
from uuid import uuid4

import duckdb
from splink import Linker

from er.config.schema import Config, Thresholds
from er.entities.ids import IdFactory, canonicalize_pair
from er.errors import ExitCode, StageFailure
from er.lake.bulk import BATCH_ROWS
from er.lake.columns import STD_RECORD_COLUMNS
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.lake.transaction import transaction
from er.matching.api import assert_no_splink_relations_in_lake, cleanup_splink, splink_api
from er.matching.evidence import build_evidence
from er.matching.model import UNIQUE_ID_COLUMN, scoring_settings
from er.matching.runtime import MatchingRuntime
from er.matching.tf import (
    STD_RECORDS_RELATION,
    assert_tf_lookup_complete,
    register_tf,
    tf_columns,
    tf_tables_path,
)
from er.matching.thresholds import in_gray_band, is_auto_merge
from er.obs.profiling import profiled
from er.obs.runctx import StageRun
from er.review.queue import GrayBandPair, upsert_gray_band_pairs, upsert_subject_relation

__all__ = [
    "BLOCKING_KEYS_RELATION",
    "MATCH_CORPUS_RELATION",
    "MATCH_SCORES_RELATION",
    "MODE_FULL",
    "RETAIN_INTERMEDIATE_KEY",
    "FullMatchResult",
    "ScoredPair",
    "ScoreSummary",
    "merge_match_scores",
    "prediction_columns",
    "review_scored_pairs",
    "score_full",
]

#: The relation this stage writes, the relation it scores, and the relation it counts
#: candidate pairs off. The third is NOT an input to scoring (S4.3.4, D2): Splink
#: regenerates its own pairs from the blocking rules, and `int_blocking_keys` is read
#: here only for the `candidate_pairs` counter.
MATCH_SCORES_RELATION: Final = "match_scores"
BLOCKING_KEYS_RELATION: Final = "int_blocking_keys"

#: The bare local relation the corpus is copied into. Bare and local because Splink
#: resolves an input table's columns through `information_schema.columns WHERE
#: table_name = '<name>'`, which a lake-qualified name matches no row of; S4.0b's
#: "materialized into local temp tables first" is the sanctioned way to get one, and
#: `er train` copies its corpus for the same reason.
MATCH_CORPUS_RELATION: Final = "er_match_corpus"

#: `runs.mode` / the `mode` counter for this stage (S4.0).
MODE_FULL: Final = "full"

#: The settings key this stage forces on; see the module docstring for why it is the
#: one deviation from the frozen artifact.
RETAIN_INTERMEDIATE_KEY: Final = "retain_intermediate_calculation_columns"

_MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.{MATCH_SCORES_RELATION}"
_STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.{STD_RECORDS_RELATION}"
_BLOCKING_KEYS: Final = f"{SCHEMA_QUALIFIER}.{BLOCKING_KEYS_RELATION}"

#: S5's column list and S5.0's logical key for `match_scores`, read from the one place
#: that declares them. The merge below is generated from both, so a relation that grows
#: a column cannot leave a hand-written statement writing NULL into it.
_COLUMNS: Final[tuple[str, ...]] = REGISTRY[MATCH_SCORES_RELATION].column_names
_KEY_COLUMNS: Final[tuple[str, ...]] = REGISTRY[MATCH_SCORES_RELATION].keys[0].columns

#: How each `match_scores` column is filled from the merge's source. The three
#: constants are constants for a reason: a row this stage writes is by definition the
#: live score for its key, so re-scoring a key that had been invalidated (S4.5.5)
#: clears the invalidation rather than leaving a row that is both current and retired.
_SOURCE_EXPRESSION: Final[Mapping[str, str]] = MappingProxyType(
    {
        "rec_a_key": "source.rec_a_key",
        "rec_b_key": "source.rec_b_key",
        "match_probability": "source.match_probability",
        "model_version": "source.model_version",
        "tf_snapshot_id": "source.tf_snapshot_id",
        "rec_a_content_hash": "source.rec_a_content_hash",
        "rec_b_content_hash": "source.rec_b_content_hash",
        "evidence": "source.evidence",
        "is_active": "true",
        "invalidated_at": "NULL",
        "invalidated_run_id": "NULL",
        "run_id": "source.run_id",
        "scored_at": "source.scored_at",
    }
)

if set(_SOURCE_EXPRESSION) != set(_COLUMNS):
    # At import, not at write time: the merge is generated from S5's column list, and
    # a column added there with no expression here would otherwise surface as a SQL
    # error inside a stage that had already scored the corpus.
    raise RuntimeError(
        f"{MATCH_SCORES_RELATION}: the merge fills "
        f"{sorted(_SOURCE_EXPRESSION)} and S5 declares {sorted(_COLUMNS)}"
    )

_CORPUS_SQL: Final = (
    f"CREATE OR REPLACE TABLE {MATCH_CORPUS_RELATION} AS "
    f"SELECT {', '.join(STD_RECORD_COLUMNS)} FROM {_STD_RECORDS}"
)

_CORPUS_COUNT_SQL: Final = f"SELECT count(*) FROM {MATCH_CORPUS_RELATION}"

#: The `candidate_pair_count` metric S4.3.4 assigns to `int_blocking_keys`: the
#: distinct canonical pairs that share at least one blocking key. `a.record_key <
#: b.record_key` is the join's own canonicalisation and its self-pair guard at once.
_CANDIDATE_PAIRS_SQL: Final = f"""
SELECT count(*) FROM (
    SELECT DISTINCT a.record_key AS rec_a_key, b.record_key AS rec_b_key
      FROM {_BLOCKING_KEYS} AS a
      JOIN {_BLOCKING_KEYS} AS b
        ON a.key_type = b.key_type
       AND a.key_value = b.key_value
       AND a.record_key < b.record_key
)
"""

#: Every row this run touched, in canonical order. Read back rather than counted in
#: SQL because the gray band is classified in Python (see the module docstring), and
#: read by `run_id` because a `MERGE` that updated an existing row stamps it with this
#: run — so the projection is exactly the set of pairs this stage scored.
_SCORED_ROWS_SQL: Final = f"""
SELECT row_number() OVER (ORDER BY rec_a_key, rec_b_key) AS batch_row,
       rec_a_key, rec_b_key, match_probability, evidence
  FROM {_MATCH_SCORES}
 WHERE model_version = ? AND tf_snapshot_id = ? AND run_id = ?
 ORDER BY rec_a_key, rec_b_key
"""


def _selected_predictions_sql(prediction_relation: str) -> str:
    """Deduplicate a local table by row ID, without aggregating evidence payloads.

    Self-pairs are dropped and endpoints are canonicalised to `rec_a_key < rec_b_key`
    before duplicate selection. Endpoint content hashes are joined on afterward.

    `least`/`greatest` are the canonicalisation, and S5.0's helper
    :func:`~er.entities.ids.canonicalize_pair` remains its authority: the helper is a
    lexical comparison of two strings, which is what DuckDB's binary collation does to
    the same two strings, and :func:`score_full` re-canonicalises every pair it reads
    back through the helper before handing it to `review_queue`. Doing the ordering in
    Python instead would mean materializing the whole scored set in the process before
    it could be written, which is the one thing the single-statement rule forbids.

    Only the winning row ID is aggregated. Aggregating the evidence JSON with
    `DISTINCT ON` retains variable-size state for every pair and can exceed the
    memory limit even when there is ample spill space. The payload is retrieved
    after selection. The caller stages a physical local table so its row IDs stay
    stable throughout this statement, including when the input was a view.

    Highest probability wins; local row ID breaks equal-probability ties. A merge
    source with duplicate logical keys would otherwise have an undefined result.
    """
    left = f"{UNIQUE_ID_COLUMN}_l"
    right = f"{UNIQUE_ID_COLUMN}_r"
    return f"""
    SELECT DISTINCT ON (rec_a_key, rec_b_key) rec_a_key, rec_b_key, prediction_id
      FROM (
            SELECT least({left}, {right}) AS rec_a_key,
                   greatest({left}, {right}) AS rec_b_key,
                   match_probability, rowid AS prediction_id
              FROM {prediction_relation}
             WHERE {left} <> {right}
           )
     ORDER BY rec_a_key, rec_b_key, match_probability DESC, prediction_id
    """


def _source_sql(prediction_relation: str, selected_relation: str) -> str:
    """Retrieve the selected payloads and canonical endpoint hashes after deduplication."""
    return f"""
    SELECT pair.rec_a_key,
           pair.rec_b_key,
           prediction.match_probability,
           ? AS model_version,
           ? AS tf_snapshot_id,
           rec_a.content_hash AS rec_a_content_hash,
           rec_b.content_hash AS rec_b_content_hash,
           prediction.evidence,
           ? AS run_id,
           CAST(? AS TIMESTAMP) AS scored_at
      FROM {selected_relation} AS pair
      JOIN {prediction_relation} AS prediction ON prediction.rowid = pair.prediction_id
      JOIN {_STD_RECORDS} AS rec_a ON rec_a.{UNIQUE_ID_COLUMN} = pair.rec_a_key
      JOIN {_STD_RECORDS} AS rec_b ON rec_b.{UNIQUE_ID_COLUMN} = pair.rec_b_key
    """


def _merge_sql(source_relation: str) -> str:
    """THE write: one `MERGE INTO` on S5.0's logical key (S4.3.4, S4.0b).

    One `WHEN MATCHED` and one `WHEN NOT MATCHED`, which is what DuckLake's `MERGE`
    supports and all this needs: a key that is already scored is rewritten in place,
    and a key that is not is inserted. There is no third branch, because there is no
    case in which this stage removes a row — `match_scores` is cumulative (S4.3.4).
    """
    predicate = " AND ".join(f"target.{column} = source.{column}" for column in _KEY_COLUMNS)
    assignments = ",\n         ".join(
        f"{column} = {_SOURCE_EXPRESSION[column]}"
        for column in _COLUMNS
        if column not in _KEY_COLUMNS
    )
    values = ", ".join(_SOURCE_EXPRESSION[column] for column in _COLUMNS)
    return f"""
MERGE INTO {_MATCH_SCORES} AS target
USING {source_relation} AS source
   ON {predicate}
 WHEN MATCHED THEN UPDATE SET
         {assignments}
 WHEN NOT MATCHED THEN INSERT ({", ".join(_COLUMNS)})
      VALUES ({values})
"""


@dataclass(frozen=True)
class FullMatchResult:
    """What one `er match --mode full` scored, in the terms S4.0 prints it in.

    The seven fields of S4.0's stdout column plus the three the S4.3.5 counter list
    adds. `candidate_pairs` is optional because `int_blocking_keys` is not an input to
    scoring (S4.3.4, D2): a lake that cannot report it still scored correctly, and
    S5.2 distinguishes "the stage does not report this" (NULL) from "the stage reports
    zero".
    """

    mode: str
    model_version: str
    tf_snapshot_id: str
    candidate_pairs: int | None
    pairs_scored: int
    pairs_above_auto_merge: int
    pairs_in_gray_band: int
    review_queue_added: int
    review_queue_refreshed: int
    rows_in: int

    @property
    def exit_code(self) -> int:
        """S4.0: ``0``. Full mode has no "nothing to do" — that exit is incremental's."""
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

        Every name S4.3.5's counter paragraph lists, plus the `rows_in`/`rows_out` pair
        S5.2 requires of every stage: records scored in, pairs persisted out.
        :meth:`~er.obs.counters.StageCounters.set` routes each name to its typed column
        or to the JSON payload, so no caller here has to know which list it is on.

        The version and the TF snapshot are also copied onto the
        :class:`~er.obs.runctx.StageRun` itself, because the S5.2 stderr record carries
        both as top-level keys — and a scored corpus whose record did not say which
        model produced it would make INV-SCORE uncheckable from the log alone.
        """
        stage_run.model_version = self.model_version
        stage_run.tf_snapshot_id = self.tf_snapshot_id
        counters = stage_run.counters
        counters.set("rows_in", self.rows_in)
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


def _scalar(connection: duckdb.DuckDBPyConnection, statement: str) -> Any:
    row = connection.execute(statement).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


@profiled("match.prepare_corpus", "records")
def _materialize_corpus(connection: duckdb.DuckDBPyConnection) -> int:
    """Copy the corpus into a bare local relation and return how many rows it holds.

    Called **after** :func:`~er.matching.api.splink_api`, which is what fixes where the
    relation lands: the API's constructor issues ``SET schema``, so an unqualified
    ``CREATE OR REPLACE TABLE`` puts the corpus in the scratch schema — the same one
    Splink resolves its own relations in — and a second scoring pass in one process
    replaces that one copy. Materializing before the API instead would put the first
    copy in `main` and the second in the scratch schema, and Splink resolves an input
    table through `information_schema.columns WHERE table_name = '<name>'`, which two
    copies match twice.

    Raises:
        er.errors.StageFailure: the corpus relation cannot be read, or holds no rows.
            Both mean `er standardize` has not run over this lake. S4.0 gives
            `--mode full` no "nothing to do" exit — that ``10`` is incremental mode's,
            for a corpus with no *unscored* records — so an empty corpus is a stage
            failure named here rather than a Splink error about an empty frame.
    """
    try:
        connection.execute(_CORPUS_SQL)
    except duckdb.Error as exc:
        raise StageFailure(
            f"{_STD_RECORDS} cannot be read: {exc}. `er match` scores the standardized "
            f"corpus, so `er standardize` runs first (S4.0, S4.3.4)"
        ) from exc
    rows = int(_scalar(connection, _CORPUS_COUNT_SQL))
    if rows == 0:
        raise StageFailure(f"{_STD_RECORDS} holds no rows; there is nothing to score (S4.3.4)")
    return rows


@profiled("match.count_candidates", "pairs")
def _candidate_pairs(connection: duckdb.DuckDBPyConnection) -> int | None:
    """The `candidate_pair_count` metric, or ``None`` when it cannot be read.

    Not a failure: `int_blocking_keys` is the blocking-rule source, the S4.5
    touched-subgraph driver and this counter, and explicitly **not** an input to
    scoring (S4.3.4, D2). A stage that scored the corpus correctly must not fail over
    a number it only reports, so an unreadable relation leaves the counter NULL.
    """
    try:
        return int(_scalar(connection, _CANDIDATE_PAIRS_SQL))
    except duckdb.Error:
        return None


def prediction_columns(connection: duckdb.DuckDBPyConnection, relation: str) -> tuple[str, ...]:
    """The column names of the prediction relation, in Splink's own order.

    Read off the relation rather than derived from the config: which columns
    `predict()` emits is a property of the settings document that produced it, and
    :func:`~er.matching.evidence.evidence_keys` is what turns the difference between
    the two into a refusal instead of a payload with a hole in it.

    Public because the S4.3.4 two-pass scorer asks the same question of two prediction
    relations — `predict_between` and `predict` need not emit the same
    columns — and answering it from the config instead is exactly the mistake the
    paragraph above rules out.
    """
    rows = connection.execute(f"DESCRIBE SELECT * FROM {relation}").fetchall()
    return tuple(str(row[0]) for row in rows)


def _stamp(moment: datetime | None) -> datetime:
    """``moment`` as the naive UTC value S5's `TIMESTAMP` columns hold."""
    stamped = datetime.now(UTC) if moment is None else moment
    return stamped if stamped.tzinfo is None else stamped.astimezone(UTC).replace(tzinfo=None)


@dataclass(frozen=True)
class ScoredPair:
    """One persisted score; decode evidence only when a reviewer needs it."""

    rec_a_key: str
    rec_b_key: str
    match_probability: float
    evidence_json: str

    @property
    def evidence(self) -> Mapping[str, Any]:
        payload: Mapping[str, Any] = json.loads(self.evidence_json)
        return payload


@dataclass
class ScoreSummary:
    """Running counters shared by full and incremental score consumption."""

    pairs_scored: int = 0
    pairs_above_auto_merge: int = 0
    pairs_in_gray_band: int = 0
    review_queue_added: int = 0
    review_queue_refreshed: int = 0


@contextmanager
def _staged_query(
    connection: duckdb.DuckDBPyConnection, query: str, parameters: list[Any] | None = None
) -> Iterator[str]:
    """Keep intermediate payloads in spillable local storage, with caller-owned transactions."""
    relation = f"temp.main.er_match_stage_{uuid4().hex}"
    connection.execute(f"CREATE TEMP TABLE {relation} AS {query}", parameters or [])
    try:
        yield relation
    except BaseException:
        # Cleanup must not hide the original failure in an aborted transaction.
        with suppress(duckdb.Error):
            connection.execute(f"DROP TABLE {relation}")
        raise
    else:
        connection.execute(f"DROP TABLE {relation}")


def _scored_rows(
    connection: duckdb.DuckDBPyConnection,
    *,
    model_version: str,
    tf_snapshot_id: str,
    run_id: str,
) -> Iterator[ScoredPair]:
    """Every pair this run scored, canonicalised through the S5.0 helper.

    The re-canonicalisation is not defensive duplication: it is what makes
    :func:`~er.entities.ids.canonicalize_pair` the authority for an ordering the merge
    had to express in SQL. A row whose keys came back in the other order — or with the
    two equal — raises here, before anything downstream joins one-sided on it.
    """
    # A native materialized result can retain the whole payload outside DuckDB's
    # managed buffer pool. A temporary table can spill and also survives interleaved
    # review writes on this same connection, including in an uncommitted transaction.
    # The ordered batch ordinal gives each read a bounded, zone-map-prunable range.
    with _staged_query(
        connection, _SCORED_ROWS_SQL, [model_version, tf_snapshot_id, run_id]
    ) as relation:
        offset = 0
        while rows := connection.execute(
            f"SELECT rec_a_key, rec_b_key, match_probability, evidence FROM {relation} "
            "WHERE batch_row > ? AND batch_row <= ? ORDER BY batch_row",
            [offset, offset + BATCH_ROWS],
        ).fetchall():
            offset += BATCH_ROWS
            for rec_a_key, rec_b_key, match_probability, evidence in rows:
                canonical = canonicalize_pair(str(rec_a_key), str(rec_b_key))
                if canonical != (str(rec_a_key), str(rec_b_key)):
                    raise StageFailure(
                        f"{MATCH_SCORES_RELATION} holds ({rec_a_key!r}, {rec_b_key!r}), which "
                        "is not the S5.0 canonical ordering; every pair relation carries "
                        "rec_a_key < rec_b_key (D9)"
                    )
                yield ScoredPair(
                    rec_a_key=canonical[0],
                    rec_b_key=canonical[1],
                    match_probability=float(match_probability),
                    evidence_json=str(evidence),
                )


@profiled("match.classify_scores", "pairs")
def review_scored_pairs(
    connection: duckdb.DuckDBPyConnection,
    scored: Iterable[ScoredPair],
    thresholds: Thresholds,
    *,
    run_id: str,
    id_factory: IdFactory | None = None,
) -> ScoreSummary:
    """Classify all scores, retaining only one batch of review payloads/results."""
    summary = ScoreSummary()

    def gray_pairs() -> Iterator[GrayBandPair]:
        for pair in scored:
            summary.pairs_scored += 1
            summary.pairs_above_auto_merge += is_auto_merge(pair.match_probability, thresholds)
            if in_gray_band(pair.match_probability, thresholds):
                yield GrayBandPair(
                    rec_a_key=pair.rec_a_key,
                    rec_b_key=pair.rec_b_key,
                    match_probability=pair.match_probability,
                    waterfall=pair.evidence,
                )

    for gray_band in batched(gray_pairs(), BATCH_ROWS):
        summary.pairs_in_gray_band += len(gray_band)
        upserted = upsert_gray_band_pairs(
            connection, gray_band, run_id=run_id, id_factory=id_factory
        )
        summary.review_queue_added += upserted.added
        summary.review_queue_refreshed += upserted.refreshed_count
    return summary


@profiled("match.classify_scores_sql", "pairs")
def review_score_relation(
    connection: duckdb.DuckDBPyConnection,
    thresholds: Thresholds,
    *,
    model_version: str,
    tf_snapshot_id: str,
    run_id: str,
    id_factory: IdFactory | None = None,
) -> ScoreSummary:
    """Classify saved scores and populate reviews without reading score payloads."""

    def compatibility() -> ScoreSummary:
        return review_scored_pairs(
            connection,
            _scored_rows(
                connection,
                model_version=model_version,
                tf_snapshot_id=tf_snapshot_id,
                run_id=run_id,
            ),
            thresholds,
            run_id=run_id,
            id_factory=id_factory,
        )

    query = (
        f"SELECT rec_a_key, rec_b_key, match_probability, evidence FROM {_MATCH_SCORES} "
        "WHERE model_version = ? AND tf_snapshot_id = ? AND run_id = ?"
    )
    with _staged_query(connection, query, [model_version, tf_snapshot_id, run_id]) as scores:
        invalid = connection.execute(
            f"SELECT rec_a_key, rec_b_key FROM {scores} WHERE rec_a_key >= rec_b_key "
            "ORDER BY rec_a_key, rec_b_key LIMIT 1"
        ).fetchone()
        if invalid is not None:
            return compatibility()
        row = connection.execute(
            f"SELECT count(*), count(*) FILTER (WHERE match_probability >= ? "
            f"AND NOT isnan(match_probability)) FROM {scores}",
            [thresholds.auto_merge],
        ).fetchone()
        assert row is not None
        summary = ScoreSummary(pairs_scored=int(row[0]), pairs_above_auto_merge=int(row[1]))
        with _staged_query(
            connection,
            "SELECT row_number() OVER (ORDER BY rec_a_key, rec_b_key) AS position, "
            "'pair'::VARCHAR AS subject_type, 'gray_band'::VARCHAR AS reason, "
            "rec_a_key, rec_b_key, NULL::VARCHAR AS entity_id, match_probability, "
            f"evidence AS waterfall FROM {scores} WHERE match_probability >= ? "
            "AND match_probability < ? AND NOT isnan(match_probability)",
            [thresholds.review_low, thresholds.auto_merge],
        ) as subjects:
            invalid = connection.execute(
                f"SELECT rec_a_key, rec_b_key, match_probability, waterfall FROM {subjects} "
                "WHERE len(list_filter(json_keys(waterfall), k -> starts_with(k, 'gamma_'))) = 0 "
                "OR len(list_filter(json_keys(waterfall), k -> starts_with(k, 'mw_') "
                "OR starts_with(k, 'bf_'))) = 0 "
                "OR waterfall IS NULL "
                "OR NOT regexp_full_match(rec_a_key, '[^:]+:[^:]+', 's') "
                "OR NOT regexp_full_match(rec_b_key, '[^:]+:[^:]+', 's') "
                "ORDER BY position LIMIT 1"
            ).fetchone()
            if invalid is not None:
                # Preserve the legacy batch-prefix behavior and diagnostic for
                # corrupt evidence; ordinary evidence is never decoded in Python.
                return compatibility()
            counted = connection.execute(f"SELECT count(*) FROM {subjects}").fetchone()
            assert counted is not None
            summary.pairs_in_gray_band = int(counted[0])
            if summary.pairs_in_gray_band:
                try:
                    summary.review_queue_added, summary.review_queue_refreshed = (
                        upsert_subject_relation(
                            connection, subjects, run_id=run_id, id_factory=id_factory
                        )
                    )
                except StageFailure:
                    # Duplicate-open diagnostics are detected before any queue
                    # writes. Replay the bounded path to preserve its prefix.
                    return compatibility()
    return summary


@profiled("match.persist_scores", "pairs")
def merge_match_scores(
    connection: duckdb.DuckDBPyConnection,
    prediction_relation: str,
    evidence_expression: str,
    *,
    model_version: str,
    tf_snapshot_id: str,
    run_id: str,
    scored_at: datetime | None = None,
    replace_active: bool = False,
    activate_tf: bool = False,
) -> Iterator[ScoredPair]:
    """THE `match_scores` write, and the rows it left behind (S4.3.4, S4.0b, S5.0).

    The `MERGE INTO` of :func:`_merge_sql` reaches the lake over a
    source that drops self-pairs, canonicalises to `rec_a_key < rec_b_key`, makes the
    result distinct on the pair and joins both endpoints' `content_hash` on. Nothing is
    deleted and nothing is truncated: `match_scores` is cumulative, so a key already
    scored is rewritten in place and a key that is not is inserted. Full replacement
    retires absent active pairs in the same transaction; correction also activates
    the journaled TF snapshot there. Historical score rows remain available.

    The returned iterator is a compatibility read-back for collection callers.
    Production classifies the written relation with :func:`review_score_relation`,
    leaving probabilities and evidence inside DuckDB.

    Both S4.3.4 scoring paths call this: full mode hands it one corpus-wide prediction,
    and the two-pass incremental scorer hands it the union of its two passes. Both
    use the same canonicalisation and score MERGE.

    Args:
        connection: an open S4.0b connection with the lake attached by alias.
        prediction_relation: the relation the merge reads, carrying `record_key_l`,
            `record_key_r`, `match_probability` and whatever ``evidence_expression``
            names. It lives in the in-memory database — a Splink prediction, or a local
            relation built from several — so the merge's source is a join across the
            two catalogs without transferring row payloads to Python.
        evidence_expression: the SQL producing each row's `evidence JSON`, from
            :func:`~er.matching.evidence.build_evidence` over
            :func:`prediction_columns` of that same relation. A caller whose relation
            already holds a materialized payload passes the column name.
        model_version: the registry version being scored at, written onto every row.
        tf_snapshot_id: the frozen TF snapshot, written onto every row.
        run_id: this stage's run, written onto every row the merge touched — which is
            what makes the read-back exactly the set this call scored.
        scored_at: the stamp every row carries; now, in UTC, when omitted.
        replace_active: retire active pairs absent from this complete prediction.
        activate_tf: activate the snapshot atomically with complete replacement.

    Returns:
        A bounded iterator over every pair this run scored, in canonical pair
        order. Evidence is decoded on access. Consume it before ending the stage.

    Raises:
        er.errors.StageFailure: a persisted pair is not in the S5.0 canonical ordering.
    """
    # Materialize only the score payload in the local catalog. This also supplies
    # stable row IDs when an incremental caller passes a view or derived relation.
    # The lake still receives exactly one atomic MERGE, with no partial score batches.
    with _staged_query(
        connection,
        f"SELECT {UNIQUE_ID_COLUMN}_l, {UNIQUE_ID_COLUMN}_r, match_probability, "
        f"{evidence_expression} AS evidence FROM {prediction_relation} "
        f"WHERE {UNIQUE_ID_COLUMN}_l <> {UNIQUE_ID_COLUMN}_r",
    ) as staged:
        # Separate the aggregate, endpoint joins and lake writer, so their
        # working sets do not have to fit in memory simultaneously.
        with _staged_query(connection, _selected_predictions_sql(staged)) as selected:
            with _staged_query(
                connection,
                _source_sql(staged, selected),
                [model_version, tf_snapshot_id, run_id, _stamp(scored_at)],
            ) as source:
                if replace_active:
                    with transaction(connection):
                        connection.execute(_merge_sql(source))
                        connection.execute(
                            f"UPDATE {_MATCH_SCORES} AS target SET is_active=false, "
                            "invalidated_at=?, invalidated_run_id=? WHERE target.is_active "
                            f"AND NOT EXISTS (SELECT 1 FROM {source} s WHERE "
                            "s.model_version=target.model_version AND "
                            "s.tf_snapshot_id=target.tf_snapshot_id AND "
                            "s.rec_a_key=target.rec_a_key AND s.rec_b_key=target.rec_b_key)",
                            [_stamp(scored_at), run_id],
                        )
                        if activate_tf:
                            connection.execute(
                                f"UPDATE {SCHEMA_QUALIFIER}.model_registry SET "
                                "tf_snapshot_id=?, tf_tables_path=? "
                                "WHERE model_version=? AND status='active'",
                                [
                                    tf_snapshot_id,
                                    tf_tables_path(model_version, tf_snapshot_id),
                                    model_version,
                                ],
                            )
                else:
                    connection.execute(_merge_sql(source))
    return _scored_rows(
        connection,
        model_version=model_version,
        tf_snapshot_id=tf_snapshot_id,
        run_id=run_id,
    )


@profiled("match.full", "pairs")
def score_full(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    run_ctx: StageRun,
    *,
    model_version: str,
    tf_snapshot_id: str,
    settings: Mapping[str, Any],
    scored_at: datetime | None = None,
    id_factory: IdFactory | None = None,
    activate_tf: bool = False,
) -> FullMatchResult:
    """Score the whole corpus and atomically replace its active scores (S4.3.4).

    The order below is the section's, and every step of it is a decision:

    1. **Build the API, copy the corpus local, then build the linker.** Splink resolves
       an input table by bare name, and the API comes from
       :func:`~er.matching.api.splink_api` — the repository's one construction site —
       so the scratch schema stays in the in-memory database and no ``__splink__``
       relation reaches the lake (M17). The corpus is materialized after the API for
       the reason :func:`_materialize_corpus` gives.
    2. **Register the frozen TF, never compute it** (D4, S4.3.3). Missing rows are a
       precondition failure, not a fallback: Splink computing TF from the corpus at
       hand would break INV-SCORE silently and for every pair.
    3. **Predict at `review_low`**, explicitly, in probabilities.
    4. **Merge once**, over a source that drops self-pairs, canonicalises, deduplicates
       and joins both endpoint content hashes on.
    5. **Classify the result in SQL** using the same half-open thresholds, and
       upsert the gray band to `review_queue` (S4.3.5) — which is an upsert with three
       outcomes, so a dismissed pair does not resurface.
    6. **Assert M17** before returning, so a leak is attributed to the stage that
       caused it rather than to whatever ran next.

    Args:
        connection: an open S4.0b connection with the lake attached by alias.
        cfg: the validated S6 document; `thresholds:` and `comparisons:` are read.
        run_ctx: this stage's `run_stages` row — the source of `run_id` and the
            destination of every counter S4.3.5 lists.
        model_version: the registry version being scored at, written onto every row.
        tf_snapshot_id: the frozen TF snapshot to register, written onto every row.
        settings: the settings document `model_version` was registered with. Passed in
            rather than fetched, because the object store is the CLI's to construct and
            a scenario test loads the committed fixture model instead (S4.3.2 item 6).
        scored_at: the stamp every row this run writes carries; now, in UTC, when
            omitted.
        id_factory: the source of minted `review_id`s (S4.5.4, D10).

    Returns:
        The S4.0 stdout fields and the S4.3.5 counters, already recorded on ``run_ctx``.

    Raises:
        er.errors.StageFailure: the corpus is unreadable or empty, the prediction
            carries no evidence columns, or a persisted pair is not canonically
            ordered. All are S4.0 exit ``1``.
        er.matching.tf.TfLookupIncomplete: the preflight found one or more `tf: true`
            columns with no frozen rows for this key. Raised before anything is built
            or written, and it names every missing column (exit ``3``).
        er.matching.tf.MissingTfLookupError: no frozen TF rows for this key (exit ``3``).
    """
    # D4 preflight, and deliberately the first statement in the function: it runs
    # before `splink_api`, before the corpus copy and before any `match_scores` write,
    # so a snapshot missing frozen rows refuses at exit 3 having written nothing.
    # `register_tf` below still raises on a column it cannot find — that guard is not
    # redundant, it is what keeps the two halves of D4 independent — but by then a
    # Linker exists and the operator learns about one missing column at a time.
    assert_tf_lookup_complete(connection, model_version, tf_snapshot_id, tf_columns(cfg))

    thresholds = cfg.thresholds
    candidate_pairs = _candidate_pairs(connection)

    # The API first, then the corpus: the constructor's `SET schema` is what decides
    # where an unqualified `CREATE OR REPLACE TABLE` lands (see `_materialize_corpus`).
    api = splink_api(connection)
    try:
        rows_in = _materialize_corpus(connection)
        linker = Linker(
            api.register(MATCH_CORPUS_RELATION), settings=scoring_settings(cfg, settings)
        )
        register_tf(linker, connection, cfg, model_version, tf_snapshot_id)
        # Explicit probability threshold: never depend on a library default.
        runtime = MatchingRuntime.from_env()
        run_ctx.counters.set("matching_runtime", runtime.fingerprint())
        predictions = linker.inference.predict(
            threshold_match_probability=thresholds.review_low,
            num_chunks_left=runtime.num_chunks_left,
            num_chunks_right=runtime.num_chunks_right,
        )
        relation = str(predictions.physical_name)

        merge_match_scores(
            connection,
            relation,
            build_evidence(cfg, prediction_columns(connection, relation)),
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            run_id=run_ctx.run_id,
            scored_at=scored_at,
            replace_active=True,
            activate_tf=activate_tf,
        )
        summary = review_score_relation(
            connection,
            thresholds,
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            run_id=run_ctx.run_id,
            id_factory=id_factory,
        )

        assert_no_splink_relations_in_lake(connection)

        result = FullMatchResult(
            mode=MODE_FULL,
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            candidate_pairs=candidate_pairs,
            pairs_scored=summary.pairs_scored,
            pairs_above_auto_merge=summary.pairs_above_auto_merge,
            pairs_in_gray_band=summary.pairs_in_gray_band,
            review_queue_added=summary.review_queue_added,
            review_queue_refreshed=summary.review_queue_refreshed,
            rows_in=rows_in,
        )
        result.record(run_ctx)
        return result
    finally:
        cleanup_splink(api)
