"""Prevent a dependency upgrade from mixing scoring engines in one lake model."""

from __future__ import annotations

import duckdb

from er.errors import PreconditionFailure
from er.lake.model_registry import ModelRow
from er.versions import PINS


def assert_matching_model(
    connection: duckdb.DuckDBPyConnection, model: ModelRow, *, incremental: bool
) -> None:
    version = PINS["splink"].version
    recorded = model.metrics.get("splink_version")
    if recorded != version:
        raise PreconditionFailure(
            f"model {model.model_version} uses Splink {recorded or 'legacy/unrecorded'}; "
            f"this engine uses {version}. Train a new model and run full matching, "
            "reconciliation and assembly before incremental processing."
        )
    if not incremental or not model.metrics.get("migration_requires_full_resolution", False):
        return
    completed = connection.execute(
        "WITH completed AS (SELECT s.*, r.model_version FROM lake.main.run_stages s "
        "JOIN lake.main.runs r USING(run_id) WHERE s.status='succeeded' "
        "AND r.model_version=?) SELECT EXISTS (SELECT 1 FROM completed m "
        "JOIN completed r ON r.stage='reconcile' AND r.started_at >= m.ended_at "
        "JOIN completed a ON a.stage='assemble' AND a.started_at >= r.ended_at "
        "WHERE m.stage='match' AND (m.counters->>'mode')='full')",
        [model.model_version],
    ).fetchone()
    if completed != (True,):
        raise PreconditionFailure(
            "Splink migration requires successful full matching, reconciliation and "
            "assembly for the new model before incremental matching."
        )
