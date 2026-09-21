"""`er assemble`: touched-only golden assembly, the reap step, and T-INC-2 (S4.6, M10).

The marts materialise `golden_records`, `golden_lineage` and `golden_display` from the
current membership. Two things S4.6 requires cannot live in dbt, and this module is
both of them plus the stage that sequences the marts between them:

* **The touched set goes through a relation, not the argv.** An incremental run
  rebuilds only the entities it changed, and the naive way to say which — a `--vars`
  payload of their ids — hard-fails with `E2BIG` once there are tens of thousands
  (M10). So :func:`compute_touched_set` derives the set from THIS run's
  `entity_events`, :func:`write_touched_entities` writes one
  `er_touched_entities(run_id, entity_id, disposition)` row each BEFORE the marts run,
  and the marts join it on `var('run_id')` (assembly/touched_entities.sql). No entity
  id ever reaches dbt.

* **The reap is explicit, because `delete+insert` cannot delete an absent key.** A
  merge loser, an emptied split fragment and a tombstoned entity are all `retire`:
  they hold no member, so the marts produce no row for them and dbt's `delete+insert`
  — which deletes only keys present in the incoming batch — leaves their stale golden
  rows behind forever. :func:`reap_retired_entities` deletes them from all three
  relations AFTER the marts return, which is the only order that keeps the run
  all-or-nothing at the logical level (S4.7): a mart failure leaves the reap unrun and
  nothing half-deleted.

`assembled_at` is the RUN's `started_at` (from the `runs` row), written only for the
entities a run touched — the untouched ones keep their rows and their stamps untouched
because `delete+insert` never names them. That is what makes T-INC-2's accounting —
`{rewritten} ∪ {reaped} == {touched}` — an equality rather than a containment.
"""

from __future__ import annotations

import time
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb

from er.config.schema import Config
from er.dbt_runner import (
    ARTIFACTS_DIR,
    DBT_PROFILES_DIR,
    DBT_PROJECT_DIR,
    DbtResult,
    render_dbt_vars,
    run_dbt,
)
from er.embeddings.coherence import get_scorer
from er.entities.ids import IdFactory
from er.errors import ExitCode, StageFailure
from er.lake.model import SCHEMA_QUALIFIER
from er.obs.profiling import profiled, span
from er.obs.runctx import ConnectionSource
from er.review.queue import upsert_entity_finding

__all__ = [
    "MARTS_SELECTOR",
    "REAP_RELATIONS",
    "TOUCHED_EVENT_TYPES",
    "AssembleResult",
    "assemble",
    "assemble_dbt_vars",
    "compute_touched_set",
    "reap_retired_entities",
    "score_touched_entities",
    "write_touched_entities",
]

#: The dbt selection `er assemble` builds — the three golden marts and nothing else.
MARTS_SELECTOR: Final = "marts"

#: The event types that make an entity part of a run's touched set (S4.6). Every way
#: an entity's membership can change in one run: minted, gained or lost a member,
#: absorbed another, broke off a fragment, emptied, or had an edge cut under it.
TOUCHED_EVENT_TYPES: Final[tuple[str, ...]] = (
    "created",
    "member_added",
    "member_removed",
    "merged",
    "split",
    "retired",
    "edge_cut",
)

#: The three golden relations the reap deletes from, in the order a reader meets them.
REAP_RELATIONS: Final[tuple[str, ...]] = ("golden_records", "golden_lineage", "golden_display")

_EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
_ENTITIES: Final = f"{SCHEMA_QUALIFIER}.entities"
_RUNS: Final = f"{SCHEMA_QUALIFIER}.runs"
_TOUCHED: Final = f"{SCHEMA_QUALIFIER}.er_touched_entities"

#: The `run_started_at` format the marts cast: a SQL timestamp literal at MICROSECOND
#: precision. Full precision is load-bearing, not cosmetic — `runs.started_at` carries
#: microseconds, so a second-truncated stamp would make `golden_records.assembled_at ==
#: runs.started_at` (AC2) false by a fraction and every rewritten entity would read as
#: neither rewritten nor reaped in T-INC-2's set accounting.
_TIMESTAMP_FORMAT: Final = "%Y-%m-%d %H:%M:%S.%f"


@profiled("assemble.touched_set", "entities")
def compute_touched_set(connection: duckdb.DuckDBPyConnection, run_id: str) -> dict[str, str]:
    """`entity_id -> disposition` for every entity this run's events touched (S4.6).

    The formula S4.6 states: an entity is touched when an event of a
    :data:`TOUCHED_EVENT_TYPES` type carries this `run_id`. Its disposition is
    `retire` when its CURRENT `entities.status` is `merged` or `retired` — a merge
    loser, an emptied split fragment, a tombstoned entity, all of which hold no member
    — and `rebuild` otherwise.

    Reading the current status rather than inferring it from the event type is what
    keeps the two consistent: an entity that split AND then had its last member
    removed in one run carries two events but one status, and it is the status that
    decides whether a golden row should exist.

    Args:
        connection: an attached lake connection.
        run_id: the run whose events define the set.

    Returns:
        `entity_id -> 'rebuild' | 'retire'`, empty when the run touched nothing.
    """
    placeholders = ", ".join("?" for _ in TOUCHED_EVENT_TYPES)
    rows = connection.execute(
        f"""
        SELECT touched.entity_id,
               CASE WHEN e.status IN ('merged', 'retired') THEN 'retire' ELSE 'rebuild' END
          FROM (
                SELECT DISTINCT entity_id FROM {_EVENTS}
                 WHERE run_id = ? AND event_type IN ({placeholders})
               ) AS touched
          LEFT JOIN {_ENTITIES} AS e ON e.entity_id = touched.entity_id
         ORDER BY touched.entity_id
        """,
        [run_id, *TOUCHED_EVENT_TYPES],
    ).fetchall()
    return {str(entity_id): str(disposition) for entity_id, disposition in rows}


def write_touched_entities(
    connection: duckdb.DuckDBPyConnection,
    run_id: str,
    touched: Mapping[str, str],
    *,
    created_at: datetime | None = None,
) -> int:
    """Write one `er_touched_entities` row per touched entity (S5, S4.6).

    Written BEFORE the marts run, because the marts select this relation back to
    learn which entities to rebuild. Idempotent within a run: the run's own rows are
    deleted first, so a re-assembled run replaces its set rather than doubling it —
    which is what makes `er assemble` re-runnable under S4.7 resume.

    Returns:
        How many rows were written.
    """
    stamp = datetime.now(UTC).replace(tzinfo=None) if created_at is None else created_at
    connection.execute(f"DELETE FROM {_TOUCHED} WHERE run_id = ?", [run_id])
    if not touched:
        return 0
    values = ", ".join("(?, ?, ?, ?)" for _ in touched)
    parameters: list[Any] = []
    for entity_id, disposition in sorted(touched.items()):
        parameters += [run_id, entity_id, disposition, stamp]
    connection.execute(
        f"INSERT INTO {_TOUCHED} (run_id, entity_id, disposition, created_at) VALUES {values}",
        parameters,
    )
    return len(touched)


@profiled("assemble.reap", "entities")
def reap_retired_entities(connection: duckdb.DuckDBPyConnection, run_id: str) -> int:
    """Delete every `retire`-disposition entity from the three golden relations (S4.6).

    Run AFTER the marts, and only after they succeed (the caller's order). One DELETE
    per relation, each keyed on this run's retire set — a merge loser has no member, so
    `delete+insert` never named it and its golden row is exactly the stale row S4.6
    says a reap must remove. History is not lost: the row's last good state is readable
    at the stage's `snapshot_start` (S5.2), which is where T-INC-2's reaped arm and
    T-SNAP-1 both read it.

    Returns:
        How many golden_records rows were reaped, which is the entity count S4.6's
        `entities_reaped` counter reports.
    """
    retire = [
        str(row[0])
        for row in connection.execute(
            f"SELECT entity_id FROM {_TOUCHED} WHERE run_id = ? AND disposition = 'retire'",
            [run_id],
        ).fetchall()
    ]
    if not retire:
        return 0
    placeholders = ", ".join("?" for _ in retire)
    for relation in REAP_RELATIONS:
        connection.execute(
            f"DELETE FROM {SCHEMA_QUALIFIER}.{relation} WHERE entity_id IN ({placeholders})",
            retire,
        )
    # The golden_records count is the entity count S4.6 reports as `entities_reaped`;
    # the other two relations are bounded by it (one display row per record, at most
    # six lineage rows), so the retire-set size IS the reaped entity count.
    return len(retire)


def assemble_dbt_vars(
    cfg: Config,
    run_id: str,
    run_started_at: datetime,
    *,
    touched_only: bool,
) -> dict[str, object]:
    """The `--vars` payload the marts receive: config, `run_id`, the run stamp, the mode.

    No entity id appears — that is the whole point of `er_touched_entities` (M10). The
    only per-run values are `run_id`, the run's `started_at` (as the timestamp literal
    the marts cast into `assembled_at`), and the touched-only flag the join reads.
    """
    return {
        **render_dbt_vars(cfg, run_id),
        "run_started_at": run_started_at.strftime(_TIMESTAMP_FORMAT),
        "assemble_touched_only": touched_only,
    }


@dataclass(frozen=True)
class AssembleResult:
    """What one `er assemble` did, in the terms S4.0 prints and S4.6 counts."""

    exit_code: int
    entities_touched: int
    entities_rebuilt: int
    entities_reaped: int
    lineage_rows: int
    tiebreak_deterministic_count: int
    duration_ms: int

    def manifest(self) -> dict[str, object]:
        """The S4.0 stdout document for this stage."""
        return {
            "stage": "assemble",
            "entities_touched": self.entities_touched,
            "entities_rebuilt": self.entities_rebuilt,
            "entities_reaped": self.entities_reaped,
            "lineage_rows": self.lineage_rows,
        }

    def stdout_line(self) -> str:
        """The human one-liner S4.0 puts beside the manifest."""
        return (
            f"assemble: {self.entities_touched} touched "
            f"({self.entities_rebuilt} rebuilt, {self.entities_reaped} reaped), "
            f"{self.lineage_rows} lineage row(s)"
        )

    def record(self, counters: Any) -> None:
        """Write the S4.6 counters onto the stage's `run_stages` row (S5.2)."""
        counters.set("entities_touched", self.entities_touched)
        counters.set("rows_in", self.entities_touched)
        counters.set("rows_out", self.entities_rebuilt)
        counters.set("input_unit", "entities")
        counters.set("output_unit", "entities")
        counters.set("entities_rebuilt", self.entities_rebuilt)
        counters.set("entities_reaped", self.entities_reaped)
        counters.set("lineage_rows", self.lineage_rows)
        counters.set("tiebreak_deterministic_count", self.tiebreak_deterministic_count)
        counters.set("duration_ms", self.duration_ms)


def _run_started_at(connection: duckdb.DuckDBPyConnection, run_id: str) -> datetime:
    """The run's `started_at` from the `runs` row — the source of `assembled_at`."""
    row = connection.execute(
        f"SELECT started_at FROM {_RUNS} WHERE run_id = ?", [run_id]
    ).fetchone()
    if row is None or row[0] is None:
        raise StageFailure(
            f"no runs row with a started_at for run_id={run_id!r}; assemble reads the "
            f"run's own stamp for assembled_at (S4.6, S5.2)"
        )
    stamp = row[0]
    return stamp if isinstance(stamp, datetime) else datetime.fromisoformat(str(stamp))


@profiled("assemble.output_counts", "lineage_rows")
def _golden_counts(connection: duckdb.DuckDBPyConnection) -> tuple[int, int]:
    """`(lineage_rows, tiebreak_deterministic_count)` after the marts (S4.6 counters)."""
    lineage = connection.execute(
        f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.golden_lineage"
    ).fetchone()
    tiebreak = connection.execute(
        f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.golden_lineage "
        "WHERE rule = 'tiebreak_deterministic'"
    ).fetchone()
    return (
        0 if lineage is None else int(lineage[0]),
        0 if tiebreak is None else int(tiebreak[0]),
    )


def score_touched_entities(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    *,
    run_id: str,
    id_factory: IdFactory | None = None,
) -> int:
    """The S11 coherence hook: score this run's rebuilt entities, queue findings (M20).

    Constructs the `coherence.scorer` once, scores the run's `disposition='rebuild'`
    entities — read from `er_touched_entities` in ascending `entity_id` order so the minted
    review ids are reproducible and the scorer sees them in input order — and writes each
    finding above the scorer's threshold as an `entity` `review_queue` row through the
    S4.3.5 upsert (refresh-open, skip-resolved). `NoopScorer` reports zero dispersion, so
    under the v1 default this runs and writes nothing. Returns how many findings were
    queued.
    """
    scorer = get_scorer(cfg)
    entity_ids = [
        str(row[0])
        for row in connection.execute(
            f"SELECT entity_id FROM {_TOUCHED} "
            "WHERE run_id = ? AND disposition = 'rebuild' ORDER BY entity_id",
            [run_id],
        ).fetchall()
    ]
    written = 0
    for coherence in scorer.score_clusters(entity_ids):
        if coherence.dispersion > scorer.threshold:
            upsert_entity_finding(
                connection,
                entity_id=coherence.entity_id,
                run_id=run_id,
                waterfall={
                    "dispersion": coherence.dispersion,
                    "outlier_record_keys": list(coherence.outlier_record_keys),
                },
                id_factory=id_factory,
            )
            written += 1
    return written


@profiled("assemble.lifecycle", "entities")
def assemble(
    source: ConnectionSource,
    cfg: Config,
    *,
    run_id: str,
    counters: Any,
    touched_only: bool,
    artifacts_dir: Path = ARTIFACTS_DIR,
    dbt: Callable[..., DbtResult] | None = None,
    id_factory: IdFactory | None = None,
) -> AssembleResult:
    """Compute the touched set, run the marts, reap, and count (S4.6, M10).

    The order is normative and every step of it is a decision some other order gets
    wrong: the touched set is written BEFORE the marts (they read it), the reap runs
    AFTER them and only if they returned (S4.7 all-or-nothing), and `run_started_at`
    is captured once from the `runs` row so every relation stamps the same instant.

    Args:
        source: a connection provider; no borrowed connection spans dbt (S4.0b).
        cfg: the validated S6 document.
        run_id: this run.
        counters: the stage's `StageCounters` (S5.2).
        touched_only: `er assemble --touched-only`. False rebuilds every active entity.
        artifacts_dir: where dbt's captured logs go.
        dbt: the dbt runner, injected for the unit seam; production uses
            :func:`~er.dbt_runner.run_dbt`.

    Returns:
        The counters and the exit code — ``10`` when `--touched-only` finds an empty
        set (S4.0), ``0`` otherwise.
    """
    started = time.monotonic()
    with span("assemble.prepare"), source() as connection:
        run_started = _run_started_at(connection, run_id)
        touched = compute_touched_set(connection, run_id) if touched_only else {}
        rebuild = {entity_id for entity_id, d in touched.items() if d == "rebuild"}

        # `--touched-only` with nothing to do is S4.0's `10`: no mart runs, no reap, no
        # golden row moves, and every `assembled_at` stays as it was.
        if touched_only and not touched:
            write_touched_entities(connection, run_id, {})
            empty = AssembleResult(
                exit_code=int(ExitCode.NOTHING_TO_DO),
                entities_touched=0,
                entities_rebuilt=0,
                entities_reaped=0,
                lineage_rows=0,
                tiebreak_deterministic_count=0,
                duration_ms=int((time.monotonic() - started) * 1000),
            )
            empty.record(counters)
            return empty

        write_touched_entities(connection, run_id, touched)

        # S11 coherence seam: score the rebuilt entities and queue findings after the touched
        # set is written and before the marts run (M20). Under the v1 `noop` scorer this writes
        # nothing and changes no golden, membership or event row.
        score_touched_entities(connection, cfg, run_id=run_id, id_factory=id_factory)

    runner = run_dbt if dbt is None else dbt
    vars_payload = assemble_dbt_vars(cfg, run_id, run_started, touched_only=touched_only)

    result = runner(
        "build",
        select=MARTS_SELECTOR,
        vars=vars_payload,
        target="lake",
        project_dir=DBT_PROJECT_DIR,
        profiles_dir=DBT_PROFILES_DIR,
        artifacts_dir=artifacts_dir,
    )
    if result.exit_code != 0:
        # The marts failed: no reap, so nothing is half-deleted (S4.7). run_dbt has
        # already raised on a non-zero exit in production, so this is the belt-and-
        # braces path for an injected runner that returns rather than raises.
        raise StageFailure(f"the golden marts exited {result.exit_code}; nothing was reaped")

    with span("assemble.finalize"), source() as connection:
        entities_reaped = reap_retired_entities(connection, run_id)
        lineage_rows, tiebreak = _golden_counts(connection)

        if touched_only:
            entities_rebuilt = len(rebuild)
            entities_touched = len(touched)
        else:
            # Full mode rebuilds every active entity and reaps nothing, so the S4.6
            # accounting `rebuilt + reaped == touched` holds with `reaped = 0` and
            # `touched = rebuilt` — the whole active corpus is what this run touched.
            entities_rebuilt = _active_entity_count(connection)
            entities_touched = entities_rebuilt

    outcome = AssembleResult(
        exit_code=int(ExitCode.SUCCESS),
        entities_touched=entities_touched,
        entities_rebuilt=entities_rebuilt,
        entities_reaped=entities_reaped,
        lineage_rows=lineage_rows,
        tiebreak_deterministic_count=tiebreak,
        duration_ms=int((time.monotonic() - started) * 1000),
    )
    outcome.record(counters)
    return outcome


def _active_entity_count(connection: duckdb.DuckDBPyConnection) -> int:
    """Active entities holding at least one member — the full-mode rebuild count."""
    row = connection.execute(
        f"SELECT count(DISTINCT entity_id) FROM {SCHEMA_QUALIFIER}.golden_records"
    ).fetchone()
    return 0 if row is None else int(row[0])
