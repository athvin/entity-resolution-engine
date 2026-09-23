"""Freeze a correction's target once and refuse competing pipeline mutations."""

import duckdb

from er.config.schema import Config
from er.errors import PreconditionFailure
from er.lake.model import SCHEMA_QUALIFIER as L
from er.lake.transaction import transaction
from er.matching.tf import materialize_tf_lookup, new_tf_snapshot_id
from er.obs.runctx import StageRun


def assert_no_pending_correction(
    connection: duckdb.DuckDBPyConnection,
    *,
    resume_run_id: str | None = None,
    assertion_repair: bool = False,
) -> None:
    """A failed correction must finish before the corpus or model can change."""
    present = connection.execute(
        "SELECT count(*) FROM information_schema.tables WHERE table_catalog='lake' "
        "AND table_schema='main' AND table_name='runs'"
    ).fetchone()
    if not present or not present[0]:
        return
    row = connection.execute(
        f"SELECT r.run_id FROM {L}.runs r WHERE r.mode='correction_pass' "
        "AND r.status <> 'succeeded' AND r.run_id IS DISTINCT FROM ? "
        f"AND (NOT ? OR EXISTS (SELECT 1 FROM {L}.run_stages s "
        "WHERE s.run_id=r.run_id AND s.stage='reconcile' AND s.status='succeeded')) "
        "ORDER BY r.started_at LIMIT 1",
        [resume_run_id, assertion_repair],
    ).fetchone()
    if row is not None:
        raise PreconditionFailure(
            f"ERR_CORRECTION_INCOMPLETE: resume with er correct --resume {row[0]}"
        )


def prepare_correction(
    connection: duckdb.DuckDBPyConnection, cfg: Config, stage: StageRun, model_version: str
) -> str:
    """Freeze TF and journal its identity together; retries never recompute it."""
    row = connection.execute(
        f"SELECT mode, model_version, tf_snapshot_id FROM {L}.runs WHERE run_id=?",
        [stage.run_id],
    ).fetchone()
    if row is None or row[0] != "correction_pass":
        raise PreconditionFailure("--new-tf-snapshot is only available through er correct")
    if row[1] != model_version:
        raise PreconditionFailure(
            "ERR_MODEL_VERSION_CHANGED: correction's model is no longer active"
        )
    if row[2] is None:
        snapshot = new_tf_snapshot_id()
        with transaction(connection):
            materialize_tf_lookup(connection, cfg, model_version, snapshot)
            connection.execute(
                f"UPDATE {L}.runs SET tf_snapshot_id=? WHERE run_id=?", [snapshot, stage.run_id]
            )
    else:
        snapshot = str(row[2])
    stage.model_version = model_version
    stage.tf_snapshot_id = snapshot
    return snapshot
