"""Delta standardization recovers partial dbt commits and removes obsolete keys."""

import csv
import os
import subprocess
from pathlib import Path

import pytest
from helpers.scenario import load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.dbt_runner import render_dbt_vars, run_dbt
from er.lake.ducklake import connect
from er.std.blocking import BLOCKING_DBT_VAR, blocking_payload
from er.std.stage import prepare_work


def command(*args):
    result = subprocess.run(["er", *args], capture_output=True, text=True, check=False)
    assert result.returncode == 0, result.stdout + result.stderr


def rows(connection, table):
    return connection.execute(f"SELECT * FROM lake.main.{table} ORDER BY ALL").fetchall()


@pytest.mark.parametrize("partial_model", ["staging", "staging int_std_records"])
def test_delta_after_partial_commit_matches_complete_rebuild(
    initialised_lake, tmp_path, partial_model
):
    scenario = load_scenario("base_10")
    drop = tmp_path / "base"
    for source, path in scenario.inputs_for("base").items():
        (drop / source).mkdir(parents=True)
        (drop / source / path.name).write_bytes(path.read_bytes())
        command("ingest", "--source", source, "--path", str(drop))
    command("standardize")
    cfg = load_config(Path(os.environ["ER_CONFIG"]))
    with connect() as connection:
        untouched = connection.execute(
            "SELECT * FROM lake.main.int_std_records WHERE source_system <> 'crm' ORDER BY ALL"
        ).fetchall()
    # One surviving CRM row now has no usable blocking values. All other CRM rows
    # become tombstones. Neither kind of change produces replacement blocking rows.
    crm = scenario.inputs_for("base")["crm"]
    with crm.open() as handle:
        reader = csv.DictReader(handle)
        header = reader.fieldnames
        original = next(reader)
    assert header is not None
    corrected = dict.fromkeys(header, "")
    corrected["crm_id"] = original["crm_id"]
    corrected["last_modified"] = "2026-09-22"
    changed = tmp_path / "changed" / "crm"
    changed.mkdir(parents=True)
    with (changed / "crm.csv").open("w") as handle:
        writer = csv.DictWriter(handle, fieldnames=header)
        writer.writeheader()
        writer.writerow(corrected)
    command("ingest", "--source", "crm", "--path", str(changed.parent), "--full-refresh-keys")
    failed_run = str(ULID())
    with connect() as connection:
        pending, delta, full_refresh = prepare_work(connection, cfg, failed_run, full_refresh=False)
    assert pending > 0 and delta and not full_refresh
    variables = render_dbt_vars(
        cfg, failed_run, {BLOCKING_DBT_VAR: blocking_payload(cfg), "standardize_delta": True}
    )
    # Model commits precede the simulated process failure. A new CLI run must still
    # finish this batch even though staging's batch watermark has already advanced.
    run_dbt("build", select=partial_model, vars=variables)
    command("standardize", "--changed-only")
    with connect() as connection:
        assert connection.execute(
            "SELECT count(*) FROM lake.main.er_standardize_work"
        ).fetchone() == (0,)
        assert connection.execute(
            "SELECT count(*) FROM lake.main.int_blocking_keys WHERE source_system='crm'"
        ).fetchone() == (0,)
        assert connection.execute(
            "SELECT source_record_id, email, phone_e164 FROM lake.main.int_std_records "
            "WHERE source_system='crm'"
        ).fetchall() == [(original["crm_id"], None, None)]
        assert (
            connection.execute(
                "SELECT * FROM lake.main.int_std_records WHERE source_system <> 'crm' ORDER BY ALL"
            ).fetchall()
            == untouched
        )
        delta_rows = {
            table: rows(connection, table) for table in ("int_std_records", "int_blocking_keys")
        }
    run_dbt(
        "build",
        select="intermediate",
        vars=render_dbt_vars(cfg, str(ULID()), {BLOCKING_DBT_VAR: blocking_payload(cfg)}),
    )
    with connect() as connection:
        for table, expected in delta_rows.items():
            assert rows(connection, table) == expected
