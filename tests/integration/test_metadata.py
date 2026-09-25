"""Unmapped fields survive full and incremental resolution without becoming evidence."""

import csv
import json
import os
import subprocess
from pathlib import Path

from helpers.cli_fixture import prepare_cli_fixture
from ulid import ULID

from er.config.loader import load_config
from er.dbt_runner import run_dbt
from er.golden.assemble import assemble_dbt_vars, materialize_touched_entities
from er.lake.ducklake import attach_statements, detach


def command(*args, allowed=(0,)):
    result = subprocess.run(["er", *args], capture_output=True, text=True, check=False)
    assert result.returncode in allowed, result.stdout + result.stderr
    return result


def deliver(root, source, rows):
    directory = root / source
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / f"{source}.csv").open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)


def test_metadata_survives_sources_duplicates_updates_and_deletion(initialised_lake, tmp_path):
    connection = initialised_lake
    root = tmp_path / "base"
    prepare_cli_fixture(connection, root)
    originals = {}
    for source in ("crm", "billing", "webforms"):
        with (root / source / f"{source}.csv").open() as handle:
            originals[source] = list(csv.DictReader(handle))
    first = originals["crm"][0]
    first_key = "crm:" + first["crm_id"]
    duplicate = {**first, "crm_id": "metadata-duplicate", "client_tier": "silver"}
    first = {**first, "client_tier": "gold"}
    changed = tmp_path / "changed"
    delivered = [first, duplicate]
    deliver(changed, "crm", delivered)
    billing = {**originals["billing"][0], "client_tier": "billing", "client_note": " Keep Case "}
    deliver(changed, "billing", [billing])

    def invoke(*args, **kwargs):
        detach(connection)
        try:
            return command(*args, **kwargs)
        finally:
            for statement in attach_statements():
                connection.execute(statement)

    def golden():
        return {
            key: json.loads(metadata)
            for key, metadata in connection.execute(
                "SELECT entity_id, metadata FROM lake.main.golden_records"
            ).fetchall()
        }

    invoke("ingest", "--source", "crm", "--path", str(changed))
    invoke("ingest", "--source", "billing", "--path", str(changed))
    invoke("standardize", "--changed-only")
    invoke("match", "--mode", "full")
    invoke("reconcile")
    invoke("assemble")
    entity = connection.execute(
        "SELECT entity_id FROM lake.main.entity_membership WHERE record_key=?", [first_key]
    ).fetchone()[0]
    initial = golden()
    assert initial[entity]["crm"] == {
        first["crm_id"]: {"client_tier": "gold"},
        "metadata-duplicate": {"client_tier": "silver"},
    }
    # Every contributing source record appears exactly once, even if it won no attribute.
    expected = {}
    for eid, source, record_id, metadata in connection.execute(
        "SELECT m.entity_id, r.source_system, r.source_record_id, r.metadata "
        "FROM lake.main.entity_membership m JOIN lake.main.int_std_records r USING(record_key)"
    ).fetchall():
        fields = json.loads(metadata)
        target = expected.setdefault(eid, {})
        if fields:
            target.setdefault(source, {})[record_id] = fields
    assert initial == expected
    assert any("billing" in metadata for metadata in initial.values())
    probabilities = connection.execute(
        "SELECT rec_a_key, rec_b_key, match_probability, evidence "
        "FROM lake.main.match_scores WHERE is_active ORDER BY 1,2"
    ).fetchall()
    membership = connection.execute(
        "SELECT record_key, entity_id FROM lake.main.entity_membership ORDER BY 1"
    ).fetchall()
    stamps = dict(
        connection.execute(
            "SELECT entity_id, assembled_at FROM lake.main.golden_records"
        ).fetchall()
    )

    # Only client metadata changes: the entity and all matching evidence stay identical.
    first["client_tier"] = "platinum"
    deliver(changed, "crm", [first, duplicate])
    run_id = str(ULID())
    invoke("ingest", "--source", "crm", "--path", str(changed), "--run-id", run_id)
    invoke("standardize", "--changed-only", "--run-id", run_id)
    invoke("match", "--mode", "incremental", "--run-id", run_id)
    invoke("reconcile", "--run-id", run_id, allowed=(0, 10))

    # Reproduce an interrupted assembly: records commit, but display never runs.
    assert connection.execute(
        "SELECT count(*) FROM lake.main.entity_events WHERE run_id=?", [run_id]
    ).fetchone() == (0,)
    assert materialize_touched_entities(connection, run_id, touched_only=True) == (1, 1)
    run_started_at = connection.execute(
        "SELECT started_at FROM lake.main.runs WHERE run_id=?", [run_id]
    ).fetchone()[0]
    variables = assemble_dbt_vars(
        load_config(Path(os.environ["ER_CONFIG"])), run_id, run_started_at, touched_only=True
    )
    detach(connection)
    try:
        run_dbt(
            "build",
            select="golden_lineage golden_records",
            vars=variables,
            artifacts_dir=tmp_path / "partial_assembly",
        )
    finally:
        for statement in attach_statements():
            connection.execute(statement)
    assert golden()[entity]["crm"][first["crm_id"]] == {"client_tier": "platinum"}
    assert connection.execute(
        "SELECT g.assembled_at, d.assembled_at FROM lake.main.golden_records g "
        "JOIN lake.main.golden_display d USING(entity_id) WHERE entity_id=?",
        [entity],
    ).fetchone() == (run_started_at, stamps[entity])

    # Retry the original run after metadata already matches; display must catch up.
    invoke("assemble", "--touched-only", "--run-id", run_id)
    assert connection.execute(
        "SELECT assembled_at FROM lake.main.golden_display WHERE entity_id=?", [entity]
    ).fetchone() == (run_started_at,)
    assert connection.execute(
        "SELECT entity_id, disposition FROM lake.main.er_touched_entities WHERE run_id=?",
        [run_id],
    ).fetchall() == [(entity, "rebuild")]
    assert golden()[entity]["crm"][first["crm_id"]] == {"client_tier": "platinum"}
    assert (
        connection.execute(
            "SELECT record_key, entity_id FROM lake.main.entity_membership ORDER BY 1"
        ).fetchall()
        == membership
    )
    assert (
        connection.execute(
            "SELECT rec_a_key, rec_b_key, match_probability, evidence "
            "FROM lake.main.match_scores WHERE is_active ORDER BY 1,2"
        ).fetchall()
        == probabilities
    )
    assert all(
        stamp == stamps[eid]
        for eid, stamp in connection.execute(
            "SELECT entity_id, assembled_at FROM lake.main.golden_records"
        ).fetchall()
        if eid != entity
    )
    incremental = golden()
    invoke("assemble")
    assert golden() == incremental
    invoke("ingest", "--source", "crm", "--path", str(changed), allowed=(0, 10))
    invoke("standardize", "--changed-only", allowed=(0, 10))
    invoke("assemble", "--touched-only", allowed=(10,))

    # A full source delivery removes the duplicate. Its metadata must disappear.
    refreshed = [{**row, "client_tier": ""} for row in originals["crm"]]
    refreshed[0] = first
    deliver(changed, "crm", refreshed)
    run_id = str(ULID())
    invoke(
        "ingest",
        "--source",
        "crm",
        "--path",
        str(changed),
        "--full-refresh-keys",
        "--run-id",
        run_id,
    )
    invoke("standardize", "--changed-only", "--run-id", run_id)
    invoke("match", "--mode", "incremental", "--run-id", run_id, allowed=(0, 10))
    invoke("reconcile", "--run-id", run_id)
    invoke("assemble", "--touched-only", "--run-id", run_id)
    assert "metadata-duplicate" not in golden()[entity]["crm"]
    final = golden()
    invoke("assemble")
    assert golden() == final
