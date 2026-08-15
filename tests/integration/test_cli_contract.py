"""T-CONC-1: single-writer enforcement at its S8.3 node id (DesignDoc.md S4.0b, S4.7).

S8.3 pins this file and this test name, and S12's M1 exit criterion names T-CONC-1
directly: the milestone cannot be signed off without "a second concurrent `er run-all`
exits 3". S13 says why it is an exit criterion rather than a nice-to-have — two writers
against one `tenant` "would interleave snapshot ranges, duplicate events, and race
`MERGE INTO` on `entity_membership`", and concurrency here is an explicit non-guarantee
rather than an untested hope.

The overlap is created by holding the lock for the length of the second invocation
rather than by racing two subprocesses. That is not a weaker test: the first writer's
whole contribution to T-CONC-1 is that it holds the tenant lock, it holds it with the
same :func:`~er.lake.catalog.tenant_lock` an `er run-all` process uses, and the second
invocation is a real `er run-all` that has no way to tell the difference. Racing two
subprocesses would assert the same thing only when the scheduler happened to overlap
them, and would silently assert nothing when it did not.

T-CFG-1 is the second S8.3 row pinned to this file: "mutate `thresholds.auto_merge`,
run `er run-all --mode incremental`: exit 3, named message, no snapshot; re-run with
`--allow-escalate`: promotes to full mode and succeeds". Both arms are here, in one
test, because the row is one guarantee — a guard that refuses and an escalation that
never rebuilds would satisfy either half alone. The wider drift matrix around it,
including the arms that need no prior run at all, is
`tests/integration/test_mode_guard.py`'s.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import Any

import duckdb
import yaml
from ulid import ULID

from er.config.hashing import config_hash
from er.config.loader import load_config
from er.lake.catalog import LOCK_HELD_MESSAGE, tenant_lock, try_advisory_lock
from er.lake.ducklake import connect, current_snapshot
from er.lake.model import SCHEMA_QUALIFIER

# `configs/test.yaml`'s tenant, which Compose supplies as `ER_CONFIG` (S7.1). A
# literal, so a wrong value in the document is a failure rather than a tautology.
TENANT = "test"


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def ledger_counts() -> tuple[int, int]:
    """`(runs, run_stages)` row counts, read on a connection of their own.

    A fresh connection because the rows under test would have been written by another
    process; a cached handle would answer with what this session last saw.
    """
    with connect() as connection:
        return (
            _count(connection, "runs"),
            _count(connection, "run_stages"),
        )


def _count(connection: duckdb.DuckDBPyConnection, relation: str) -> int:
    row = connection.execute(f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{relation}").fetchone()
    return 0 if row is None else int(row[0])


def stage_records(stderr: str) -> list[dict[str, Any]]:
    """The S5.2 records on stderr, in emission order."""
    return [json.loads(line) for line in stderr.splitlines() if line.startswith("{")]


def test_second_concurrent_run_exits_3(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """T-CONC-1/AC2: the second writer exits 3, names the first, and writes nothing."""
    first_run_id = str(ULID())
    before = ledger_counts()

    with tenant_lock(TENANT, run_id=first_run_id):
        second = run_er("run-all", "--mode", "incremental", "--skip-ingest")

    assert second.returncode == 3, second.stdout + second.stderr
    # S4.7's literal message, rendered from its one definition, naming the tenant that
    # is contended and the run that holds it.
    assert LOCK_HELD_MESSAGE.format(tenant=TENANT, run_id=first_run_id) in second.stderr
    assert f"writer lock held for tenant {TENANT} by run {first_run_id}" in second.stderr

    # "without writing a `runs` row" (S8.3). Not one row, and not one stage row either:
    # the refusal happens before the RunContext is entered, so there is no ledger entry
    # for a run that never ran and no stage record claiming one began.
    assert ledger_counts() == before
    assert stage_records(second.stderr) == []
    assert second.stdout == "", "a refused writer emitted command output"


def test_the_refusal_leaves_the_tenant_unlocked(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC3: refusing does not wedge the namespace, and the writer that held it can go.

    The failure this guards against is a contender that takes the lock, discovers it
    is contended, and unlocks — releasing the *holder's* lock, because `pg_advisory_unlock`
    is keyed on the value rather than on the acquisition.
    """
    with tenant_lock(TENANT, run_id=str(ULID())):
        refused = run_er("run-all", "--mode", "incremental", "--skip-ingest")
        assert refused.returncode == 3, refused.stdout + refused.stderr
        assert try_advisory_lock(TENANT) is False, "the refused writer released the holder's lock"

    assert try_advisory_lock(TENANT) is True
    after = run_er("run-all", "--mode", "incremental", "--skip-ingest")
    assert after.returncode == 0, after.stdout + after.stderr


def test_incremental_refuses_on_config_drift(
    initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    """T-CFG-1/AC7: config drift is caught, not absorbed — and escalation rebuilds.

    The prior run is a real `er run-all`, not a seeded row: "the last successful run
    for this tenant" (S4.0) has to be a run the CLI itself recorded, or the guard could
    be reading a baseline no invocation would ever produce.
    """
    baseline = run_er("run-all", "--mode", "incremental", "--skip-ingest")
    assert baseline.returncode == 0, baseline.stdout + baseline.stderr
    on_disk_hash = config_hash(load_config(Path(os.environ["ER_CONFIG"])))

    # `thresholds.auto_merge`, the field S8.3 names. `0.96` keeps S6's
    # `0 < review_low < auto_merge <= 1` satisfied, so what the run meets is the drift
    # guard and not a validation error that would also exit non-zero.
    document = yaml.safe_load(Path(os.environ["ER_CONFIG"]).read_text(encoding="utf-8"))
    document["thresholds"]["auto_merge"] = 0.96
    edited = tmp_path / "drifted.yaml"
    edited.write_text(yaml.safe_dump(document, sort_keys=False), encoding="utf-8")
    drifted_hash = config_hash(load_config(edited))
    assert drifted_hash != on_disk_hash, "the edit did not move the config_hash"

    with connect() as connection:
        snapshot_before = current_snapshot(connection)

    refused = run_er("run-all", "--mode", "incremental", "--skip-ingest", "--config", str(edited))

    assert refused.returncode == 3, refused.stdout + refused.stderr
    assert "config_hash" in refused.stderr
    assert on_disk_hash in refused.stderr and drifted_hash in refused.stderr
    # "no snapshot": the refusal is an S4.7 `precondition`, so nothing was committed
    # and no stage announced itself.
    with connect() as connection:
        assert current_snapshot(connection) == snapshot_before
    assert stage_records(refused.stderr) == []

    escalated = run_er(
        "run-all",
        "--mode",
        "incremental",
        "--skip-ingest",
        "--allow-escalate",
        "--config",
        str(edited),
    )

    assert escalated.returncode == 0, escalated.stdout + escalated.stderr
    escalated_run_id = {record["run_id"] for record in stage_records(escalated.stderr)}
    assert len(escalated_run_id) == 1
    with connect() as connection:
        # "promotes to full mode": recorded as one, so `run_stages` describes the chain
        # that actually ran (S5.2).
        assert connection.execute(
            f"SELECT mode, status FROM {SCHEMA_QUALIFIER}.runs WHERE run_id = ?",
            [escalated_run_id.pop()],
        ).fetchall() == [("full", "succeeded")]
