"""SQL affected-set discovery and reconciliation; graph cuts are an explicit seam."""

from __future__ import annotations

import time
from collections.abc import Sequence
from contextlib import ExitStack
from datetime import UTC, datetime
from typing import TYPE_CHECKING, Any
from uuid import uuid4

import duckdb

from er.config.schema import Config
from er.entities.cluster import label_propagate_relations, last_reconciled_watermark
from er.entities.events import append_events
from er.entities.ids import IdFactory
from er.entities.relational import event_stream, lifecycle_plan, scalar
from er.errors import ExitCode
from er.lake.bulk import staged_query, staged_rows
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.edges import _canonical, materialize_current_edges
from er.obs.runctx import StageRun
from er.review.assertions import Assertion
from er.review.never_cut import CutResult, never_cut_fixpoint, persist_cuts
from er.review.queue import RESOLVED_STATUSES, upsert_subject_relation

if TYPE_CHECKING:
    from er.entities.reconcile_stage import ReconcileResult

L = SCHEMA_QUALIFIER


def seed_query(run_id: str, watermark: datetime | None) -> tuple[str, list[Any]]:
    """The five seed arms, including entity reviews and resurrection history."""
    parameters: list[Any] = [run_id]

    def since(column: str) -> str:
        if watermark is None:
            return "TRUE"
        parameters.append(watermark)
        return f"{column} > ?"

    raw = f"{L}.raw_records"
    query = (
        f"SELECT source_system || ':' || source_record_id AS record_key FROM {raw} "
        f"WHERE ingest_batch_id IN (SELECT ingest_batch_id FROM "
        f"{L}.ingest_batches WHERE run_id = ?) "
        f"UNION SELECT source_system || ':' || source_record_id FROM {raw} a "
        f"WHERE NOT a.is_deleted AND {since('a.ingested_at')} AND EXISTS "
        f"(SELECT 1 FROM {raw} p WHERE p.source_system = a.source_system "
        "AND p.source_record_id = a.source_record_id AND p.content_hash <> a.content_hash "
        "AND p.ingested_at <= a.ingested_at) "
        f"UNION SELECT source_system || ':' || source_record_id FROM {raw} a "
        f"WHERE a.is_deleted AND {since('a.ingested_at')} "
        f"UNION SELECT source_system || ':' || source_record_id FROM {raw} a "
        f"WHERE NOT a.is_deleted AND {since('a.ingested_at')} AND EXISTS "
        f"(SELECT 1 FROM {raw} p WHERE p.source_system = a.source_system "
        "AND p.source_record_id = a.source_record_id AND p.is_deleted "
        "AND p.ingested_at < a.ingested_at) "
        "UNION SELECT unnest([rec_a_key, rec_b_key]) "
        f"FROM {L}.assertions WHERE {since('created_at')} OR "
        f"(retracted_at IS NOT NULL AND {since('retracted_at')}) "
    )
    statuses = ", ".join("'" + status + "'" for status in sorted(RESOLVED_STATUSES))
    query += (
        "UNION SELECT unnest([rec_a_key, rec_b_key]) "
        f"FROM {L}.review_queue WHERE subject_type = 'pair' AND status IN ({statuses}) "
        f"AND resolved_at IS NOT NULL AND {since('resolved_at')} "
        f"UNION SELECT m.record_key FROM {L}.entity_membership m "
        f"WHERE entity_id IN (SELECT entity_id FROM {L}.review_queue "
        f"WHERE subject_type = 'entity' AND entity_id IS NOT NULL AND status IN ({statuses}) "
        f"AND resolved_at IS NOT NULL AND {since('resolved_at')})"
    )
    return query, parameters


def run_affected_reconcile(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    run_ctx: StageRun,
    *,
    model_version: str,
    tf_snapshot_id: str,
    assertions: Sequence[Assertion],
    ids: IdFactory,
    occurred_at: datetime | None,
    reason: str | None,
    started: float,
    full: bool = False,
) -> ReconcileResult:
    from er.entities.reconcile_stage import (
        ReconcileResult,
        _merge_entity_relation,
        _merge_membership_relation,
        _nothing_to_do,
    )

    stamp = datetime.now(UTC).replace(tzinfo=None) if occurred_at is None else occurred_at
    with ExitStack() as stack:

        def stage(sql: str, parameters: Sequence[Any] = ()) -> str:
            return stack.enter_context(staged_query(connection, sql, parameters))

        view = f"er_affected_edges_{uuid4().hex}"
        materialize_current_edges(
            connection, model_version, tf_snapshot_id, name=view, materialized=True
        )

        # Drop before returning/raising, including when the caller's transaction aborts.
        def drop_edges() -> None:
            from contextlib import suppress

            with suppress(duckdb.Error):
                connection.execute(f"DROP TABLE IF EXISTS {view}")

        stack.callback(drop_edges)
        invalid = connection.execute(
            f"SELECT rec_a_key, rec_b_key FROM {view} WHERE rec_a_key >= rec_b_key "
            "ORDER BY rec_a_key, rec_b_key LIMIT 1"
        ).fetchone()
        if invalid is not None:
            _canonical(str(invalid[0]), str(invalid[1]))
        seeds = (
            stage(
                f"SELECT record_key FROM {L}.int_std_records UNION "
                f"SELECT record_key FROM {L}.entity_membership"
            )
            if full
            else stage(*seed_query(run_ctx.run_id, last_reconciled_watermark(connection)))
        )
        adjusted = stage(
            f"SELECT e.rec_a_key, e.rec_b_key, e.match_probability FROM {view} e "
            f"WHERE NOT EXISTS (SELECT 1 FROM {L}.assertions a WHERE a.active "
            "AND a.rec_a_key = e.rec_a_key AND a.rec_b_key = e.rec_b_key) "
            f"UNION ALL SELECT a.rec_a_key, a.rec_b_key, 1.0 FROM {L}.assertions a "
            "WHERE a.active AND a.kind = 'always' AND NOT EXISTS "
            f"(SELECT 1 FROM {L}.assertions n WHERE n.active AND n.kind = 'never' "
            "AND n.rec_a_key = a.rec_a_key AND n.rec_b_key = a.rec_b_key)"
        )
        # Close over the graph BEFORE cuts, plus old memberships (including deleted
        # nodes). A new path may invalidate an old cut in a previously split entity.
        all_nodes = stage(f"SELECT record_key FROM {seeds}")
        has_cuts = scalar(connection, f"SELECT EXISTS (SELECT 1 FROM {L}.cut_edges WHERE active)")
        if not full and not has_cuts and not any(a.kind == "never" for a in assertions):
            # With no cuts, each prior entity is a whole threshold component.
            # One scored-partner join plus prior membership is the exact closure.
            reached = stage(
                f"SELECT record_key FROM {seeds} UNION "
                f"SELECT e.rec_b_key FROM {adjusted} e JOIN {seeds} s "
                "ON s.record_key=e.rec_a_key WHERE e.match_probability>=? UNION "
                f"SELECT e.rec_a_key FROM {adjusted} e JOIN {seeds} s "
                "ON s.record_key=e.rec_b_key WHERE e.match_probability>=?",
                [cfg.thresholds.auto_merge, cfg.thresholds.auto_merge],
            )
            all_nodes = stage(
                f"SELECT record_key FROM {reached} UNION SELECT p.record_key "
                f"FROM {L}.entity_membership p WHERE p.entity_id IN (SELECT m.entity_id "
                f"FROM {L}.entity_membership m JOIN {reached} USING(record_key))"
            )
        elif not full:
            while True:
                with staged_query(
                    connection,
                    f"SELECT record_key FROM {all_nodes} UNION "
                    f"SELECT e.rec_b_key FROM {adjusted} e JOIN {all_nodes} s "
                    "ON s.record_key=e.rec_a_key WHERE e.match_probability >= ? UNION "
                    f"SELECT e.rec_a_key FROM {adjusted} e JOIN {all_nodes} s "
                    "ON s.record_key=e.rec_b_key WHERE e.match_probability >= ? UNION "
                    f"SELECT p.record_key FROM {L}.entity_membership p WHERE p.entity_id IN "
                    f"(SELECT m.entity_id FROM {L}.entity_membership m "
                    f"JOIN {all_nodes} n USING(record_key))",
                    [cfg.thresholds.auto_merge, cfg.thresholds.auto_merge],
                ) as expanded:
                    unchanged = scalar(connection, f"SELECT count(*) FROM {expanded}") == scalar(
                        connection, f"SELECT count(*) FROM {all_nodes}"
                    )
                    if not unchanged:
                        connection.execute(f"DELETE FROM {all_nodes}")
                        connection.execute(f"INSERT INTO {all_nodes} SELECT * FROM {expanded}")
                    if unchanged:
                        break
        entities = stage(
            f"SELECT DISTINCT entity_id FROM {L}.entity_membership "
            f"JOIN {all_nodes} USING(record_key)"
        )
        prior = stage(f"SELECT m.* FROM {L}.entity_membership m JOIN {entities} USING(entity_id)")
        affected_count = scalar(connection, f"SELECT count(*) FROM {all_nodes}")
        if not affected_count:
            run_ctx.counters.set("rows_in", 0)
            run_ctx.counters.set("rows_out", 0)
            result = _nothing_to_do()
            result.record(run_ctx, duration_ms=int((time.monotonic() - started) * 1000))
            return result
        departed = stage(
            f"SELECT p.entity_id, p.record_key FROM {prior} p "
            f"ANTI JOIN {L}.int_std_records s USING(record_key)"
        )
        if scalar(connection, f"SELECT EXISTS (SELECT 1 FROM {departed})"):
            connection.execute(
                f"DELETE FROM {L}.entity_membership WHERE record_key IN "
                f"(SELECT record_key FROM {departed})"
            )
        old = stage(
            f"SELECT p.entity_id, p.record_key FROM {prior} p ANTI JOIN "
            f"{departed} USING(record_key)"
        )
        nodes = stage(
            f"SELECT n.record_key FROM {all_nodes} n SEMI JOIN "
            f"{L}.int_std_records USING(record_key)"
        )
        # Derive cuts afresh; historical cuts cannot constrain this search.
        edges = stage(
            "SELECT e.rec_a_key, e.rec_b_key, e.match_probability "
            f"FROM {view} e JOIN {nodes} a ON a.record_key=e.rec_a_key "
            f"JOIN {nodes} b ON b.record_key=e.rec_b_key WHERE e.match_probability >= ? "
            f"AND NOT EXISTS (SELECT 1 FROM {L}.assertions s WHERE s.active "
            "AND s.rec_a_key=e.rec_a_key AND s.rec_b_key=e.rec_b_key) "
            "UNION ALL SELECT s.rec_a_key, s.rec_b_key, 1.0 "
            f"FROM {L}.assertions s JOIN {nodes} a ON a.record_key=s.rec_a_key "
            f"JOIN {nodes} b ON b.record_key=s.rec_b_key WHERE s.active AND s.kind='always' "
            f"AND NOT EXISTS (SELECT 1 FROM {L}.assertions n WHERE n.active AND n.kind='never' "
            "AND n.rec_a_key=s.rec_a_key AND n.rec_b_key=s.rec_b_key)",
            [cfg.thresholds.auto_merge],
        )

        def cluster() -> tuple[str, int]:
            with label_propagate_relations(
                connection, nodes, edges, max_iterations=cfg.clustering.max_iterations
            ) as (labels, iterations):
                return stage(f"SELECT * FROM {labels}"), iterations

        labels, iterations = cluster()
        violating = stage(
            f"SELECT DISTINCT a.label FROM {L}.assertions s "
            f"JOIN {labels} a ON a.record_key=s.rec_a_key "
            f"JOIN {labels} b ON b.record_key=s.rec_b_key "
            "WHERE s.active AND s.kind='never' AND a.label=b.label"
        )
        cut = CutResult()
        if scalar(connection, f"SELECT count(*) FROM {violating}"):
            # Deliberate Python graph seam: fetch only components with a live never
            # violation, in the same global order used by the reference cut loop.
            graph = connection.execute(
                f"SELECT e.rec_a_key, e.rec_b_key, e.match_probability FROM {edges} e "
                f"JOIN {labels} n ON n.record_key=e.rec_a_key "
                f"SEMI JOIN {violating} USING(label) ORDER BY e.rec_a_key, e.rec_b_key"
            ).fetchall()
            cut = never_cut_fixpoint(
                graph,
                assertions,
                cut_protect_probability=cfg.clustering.cut_protect_probability,
                max_iterations=cfg.clustering.max_iterations,
            )
            if cut.cuts:
                with staged_rows(
                    connection,
                    (("a", "VARCHAR"), ("b", "VARCHAR")),
                    (c.pair for c in cut.cuts),
                ) as removed:
                    connection.execute(
                        f"DELETE FROM {edges} e USING {removed} r "
                        "WHERE e.rec_a_key=r.a AND e.rec_b_key=r.b"
                    )
                labels, iterations = cluster()
        with staged_rows(
            connection,
            (
                ("rec_a_key", "VARCHAR"),
                ("rec_b_key", "VARCHAR"),
                ("probability", "DOUBLE"),
                ("assertion_id", "VARCHAR"),
            ),
            ((c.rec_a_key, c.rec_b_key, c.match_probability, c.assertion_id) for c in cut.cuts),
        ) as derived:
            identity = (
                "d.rec_a_key=c.rec_a_key AND d.rec_b_key=c.rec_b_key AND "
                "d.assertion_id=c.assertion_id AND d.probability=c.match_probability AND "
                "c.model_version=? AND c.tf_snapshot_id=?"
            )
            released = connection.execute(
                f"UPDATE {L}.cut_edges c SET active=false, released_run_id=?, released_at=? "
                f"WHERE c.active AND (c.rec_a_key IN (SELECT record_key FROM {all_nodes}) "
                f"OR c.rec_b_key IN (SELECT record_key FROM {all_nodes})) "
                f"AND NOT EXISTS (SELECT 1 FROM {derived} d WHERE {identity})",
                [run_ctx.run_id, stamp, model_version, tf_snapshot_id],
            ).fetchone()
            assert released is not None
            cuts_released = int(released[0])
            retained = {
                (str(a), str(b))
                for a, b in connection.execute(
                    f"SELECT d.rec_a_key, d.rec_b_key FROM {derived} d "
                    f"JOIN {L}.cut_edges c ON {identity} WHERE c.active",
                    [model_version, tf_snapshot_id],
                ).fetchall()
            }
        fresh_cuts = tuple(c for c in cut.cuts if c.pair not in retained)
        with lifecycle_plan(connection, old, entities, labels, ids) as plan:
            cut_ids = {c.pair: ids.new() for c in fresh_cuts}
            cuts = stack.enter_context(
                staged_rows(
                    connection,
                    (
                        ("position", "BIGINT"),
                        ("rec_a_key", "VARCHAR"),
                        ("rec_b_key", "VARCHAR"),
                        ("probability", "DOUBLE"),
                        ("assertion_id", "VARCHAR"),
                        ("cut_id", "VARCHAR"),
                    ),
                    (
                        (
                            i,
                            c.rec_a_key,
                            c.rec_b_key,
                            c.match_probability,
                            c.assertion_id,
                            cut_ids[c.pair],
                        )
                        for i, c in enumerate(fresh_cuts, 1)
                    ),
                )
            )
            removals = stage(
                "SELECT entity_id, 'member_removed' AS event_type, "
                "json_object('member_keys', list(record_key ORDER BY record_key), "
                "'cause', 'tombstone') AS details, row_number() OVER (ORDER BY "
                "entity_id) AS position "
                f"FROM {departed} GROUP BY entity_id"
            )
            removal_count = scalar(connection, f"SELECT count(*) FROM {removals}")
            plan_count = scalar(connection, f"SELECT count(*) FROM {plan.events}")
            all_events = stage(
                f"SELECT * FROM {plan.events} UNION ALL "
                f"SELECT entity_id, event_type, details, position + {plan_count} FROM {removals} "
                "UNION ALL SELECT p.entity_id, 'edge_cut', "
                "json_object('rec_a_key', c.rec_a_key, 'rec_b_key', c.rec_b_key, "
                "'match_probability', c.probability, 'assertion_id', "
                "c.assertion_id, 'cut_id', c.cut_id), "
                f"c.position + {plan_count + removal_count} FROM {cuts} c "
                f"JOIN {plan.placement} p ON p.record_key=c.rec_a_key"
            )
            entities_changed = scalar(connection, f"SELECT EXISTS (SELECT 1 FROM {plan.entities})")
            membership_changed = scalar(
                connection, f"SELECT EXISTS (SELECT 1 FROM {plan.membership})"
            )
            if entities_changed:
                _merge_entity_relation(connection, plan.entities, stamp, run_ctx.run_id)
            if membership_changed:
                _merge_membership_relation(connection, plan.membership, stamp, run_ctx.run_id)
            written = append_events(
                connection,
                event_stream(connection, all_events, run_id=run_ctx.run_id, ids=ids, reason=reason),
                occurred_at=stamp,
            )
            cuts_written = persist_cuts(
                connection,
                fresh_cuts,
                run_id=run_ctx.run_id,
                model_version=model_version,
                tf_snapshot_id=tf_snapshot_id,
                cut_at=occurred_at,
                cut_ids=cut_ids,
            )
            if cut.escalations:
                subjects = stack.enter_context(
                    staged_rows(
                        connection,
                        (
                            ("position", "BIGINT"),
                            ("subject_type", "VARCHAR"),
                            ("reason", "VARCHAR"),
                            ("rec_a_key", "VARCHAR"),
                            ("rec_b_key", "VARCHAR"),
                            ("entity_id", "VARCHAR"),
                            ("match_probability", "DOUBLE"),
                            ("waterfall", "JSON"),
                        ),
                        (
                            (i, "pair", "never_unsatisfiable", a, b, None, None, None)
                            for i, (a, b, _) in enumerate(cut.escalations, 1)
                        ),
                    )
                )
                upsert_subject_relation(connection, subjects, run_id=run_ctx.run_id, id_factory=ids)
            result = ReconcileResult(
                exit_code=int(
                    ExitCode.SUCCESS
                    if any(
                        (
                            entities_changed,
                            membership_changed,
                            removal_count,
                            written,
                            cuts_written,
                            cuts_released,
                            cut.escalations,
                        )
                    )
                    else ExitCode.NOTHING_TO_DO
                ),
                affected_entities=scalar(connection, f"SELECT count(*) FROM {entities}"),
                affected_edges=scalar(connection, f"SELECT count(*) FROM {edges}"),
                label_prop_iterations=iterations,
                clusters_out=plan.clusters,
                entities_created=plan.created,
                entities_merged=plan.merged,
                entities_split=plan.split,
                entities_retired=plan.retired,
                members_added=plan.added,
                members_removed=plan.removed + removal_count,
                events_emitted=written,
                edges_cut=cuts_written,
                cut_iterations=cut.iterations,
                never_unsatisfiable_escalations=len(cut.escalations),
            )
        run_ctx.counters.set("rows_in", affected_count)
        run_ctx.counters.set("rows_out", result.clusters_out)
        run_ctx.counters.set("input_unit", "records")
        run_ctx.counters.set("output_unit", "entities")
        result.record(run_ctx, duration_ms=int((time.monotonic() - started) * 1000))
        return result
