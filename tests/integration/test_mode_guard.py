"""The S4.0 config-drift guard, end to end (DesignDoc.md S4.0, S4.7, S5.1, S5.2).

S4.0: "`er run-all --mode incremental` refuses to proceed (exit `3`) when
`(config_hash, model_version, std_version)` differ from the last successful run for
this tenant; `--allow-escalate` promotes the run to `--mode full` instead of failing."
S5.1 adds the fourth field and says why the guard exists at all: a `std_version` or
`survivorship_version` bump "invalidates the derived corpus", the affected stages MUST
then run non-incrementally, and "a version bump is exactly the drift the S4.0
config-drift guard catches".

The decision itself is pure and its matrix is `tests/unit/test_mode_preconditions.py`'s
(AC3, AC6). What needs a lake, and is therefore here, is everything the decision is
*about*: that a refusal leaves `runs`, `run_stages` and the snapshot version exactly as
it found them (S4.7's `precondition` class writes nothing), that an escalated run is
recorded as a full run and executes the full chain, that a first run and a failed prior
run are not baselines, and that `--mode full` is never guarded.

T-CFG-1 is the S8.3-pinned node of the same behaviour and lives in
`tests/integration/test_cli_contract.py`, where S8.3 pins it. This file is the wider
matrix around it.
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

from er.cli import run_all_chain
from er.config.hashing import config_hash
from er.config.loader import load_config
from er.lake.ducklake import connect, current_snapshot
from er.lake.model import SCHEMA_QUALIFIER
from er.obs.runctx import RunContext, held
from er.versions import MODE_FULL, MODE_INCREMENTAL, last_successful_run

#: `configs/test.yaml`'s tenant, which Compose supplies as `ER_CONFIG` (S7.1). A
#: literal, so a wrong value in the document is a failure rather than a tautology.
TENANT = "test"

#: The four stages `er run-all --skip-ingest` chains, in order (S4.0). The same four
#: in both modes — only the flags differ, which is why AC2 asserts the flags against
#: :func:`~er.cli.run_all_chain` rather than against `run_stages`, whose S5 columns
#: record a stage's name and never its arguments.
CHAIN_STAGES = ("standardize", "match", "reconcile", "assemble")

#: A fingerprint no document on disk can hash to, for seeding a prior run that has
#: drifted in every field. `model_version` is included because it is the one field of
#: the four that no config edit can move (S5).
SEEDED_CONFIG_HASH = "f" * 64
SEEDED_MODEL_VERSION = "v0001"
SEEDED_STD_VERSION = "99"
SEEDED_SURVIVORSHIP_VERSION = "99"


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def er_run_all(
    *extra: str, config: Path | None = None, mode: str = MODE_INCREMENTAL
) -> subprocess.CompletedProcess[str]:
    """`er run-all --mode <mode> --skip-ingest`, the invocation every case here makes."""
    args = ["run-all", "--mode", mode, "--skip-ingest", *extra]
    if config is not None:
        args += ["--config", str(config)]
    return run_er(*args)


def config_path() -> Path:
    """The S6 document Compose supplies, which is what the CLI will hash (S7.1)."""
    return Path(os.environ["ER_CONFIG"])


def drifted_config(tmp_path: Path, **thresholds: float) -> Path:
    """The repo's own document with `thresholds` edited, written where the CLI can read it.

    `thresholds.auto_merge` is the field AC1 and T-CFG-1 both name. It is in the hashed
    canonical form and changes nothing else about the run, so what the guard sees is a
    moved `config_hash` and not a second difference it might have been reacting to.
    """
    document = yaml.safe_load(config_path().read_text(encoding="utf-8"))
    document["thresholds"].update(thresholds)
    edited = tmp_path / "drifted.yaml"
    edited.write_text(yaml.safe_dump(document, sort_keys=False), encoding="utf-8")
    return edited


def stage_records(stderr: str) -> list[dict[str, Any]]:
    """The S5.2 records on stderr, in emission order."""
    return [json.loads(line) for line in stderr.splitlines() if line.startswith("{")]


def run_id_of(result: subprocess.CompletedProcess[str]) -> str:
    """The one `run_id` an `er run-all` threaded through every stage (S4.0)."""
    identifiers = {record["run_id"] for record in stage_records(result.stderr)}
    assert len(identifiers) == 1, f"expected one run_id, got {sorted(identifiers)}"
    return str(identifiers.pop())


def query(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: Any) -> list[Any]:
    return connection.execute(sql, list(parameters)).fetchall()


def ledger() -> tuple[list[Any], list[Any], int]:
    """`(runs rows, run_stages rows, snapshot version)`, on a connection of their own.

    A fresh connection because the rows under test are written by another process; a
    handle this session already holds would answer with what it last saw, and "the
    snapshot version is unchanged" would then be true of a cache rather than of the
    lake.
    """
    with connect() as connection:
        return (
            query(
                connection,
                f"SELECT run_id, mode, status FROM {SCHEMA_QUALIFIER}.runs ORDER BY run_id",
            ),
            query(
                connection,
                f"SELECT run_id, stage, seq, status FROM {SCHEMA_QUALIFIER}.run_stages "
                f"ORDER BY run_id, seq",
            ),
            current_snapshot(connection),
        )


def seed_prior_run(
    connection: duckdb.DuckDBPyConnection,
    *,
    status_succeeded: bool,
    config_hash_value: str = SEEDED_CONFIG_HASH,
) -> str:
    """Leave one prior run in the ledger, drifted in all four fingerprint fields.

    Driven through :class:`~er.obs.runctx.RunContext` rather than INSERTed, so the row
    the guard reads is written by the one writer of `runs` (S5.2) and carries whatever
    that writer decides — a hand-built row could satisfy the guard's SELECT while
    disagreeing with what the CLI would actually have recorded.
    """
    run_id = str(ULID())
    context = RunContext(
        run_id=run_id,
        mode=MODE_INCREMENTAL,
        tenant=TENANT,
        config_hash=config_hash_value,
        model_version=SEEDED_MODEL_VERSION,
        std_version=SEEDED_STD_VERSION,
        survivorship_version=SEEDED_SURVIVORSHIP_VERSION,
        source=held(connection),
    )
    with context as run, run.stage("standardize") as stage_run:
        stage_run.finish(0 if status_succeeded else 1)
    return run_id


def test_config_hash_drift_refuses_with_exit_3(
    initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    """AC1: exit 3, both values named, and a ledger and a snapshot left untouched."""
    baseline = er_run_all()
    assert baseline.returncode == 0, baseline.stdout + baseline.stderr
    on_disk_hash = config_hash(load_config(config_path()))

    edited = drifted_config(tmp_path, auto_merge=0.96)
    drifted_hash = config_hash(load_config(edited))
    assert drifted_hash != on_disk_hash, "the edit did not move the config_hash"

    runs_before, stages_before, snapshot_before = ledger()
    refused = er_run_all(config=edited)

    assert refused.returncode == 3, refused.stdout + refused.stderr
    # "a message naming `config_hash` and both values" — the operator's next act is
    # either to accept a rebuild or to undo the edit, and neither is decidable from
    # "the config changed".
    assert "config_hash" in refused.stderr
    assert on_disk_hash in refused.stderr
    assert drifted_hash in refused.stderr

    # S4.7's `precondition` class writes nothing: the guard runs after the lock and
    # before the first stage, so there is no `runs` row for a run that never ran, no
    # `run_stages` row claiming a stage began, and no committed snapshot.
    runs_after, stages_after, snapshot_after = ledger()
    assert runs_after == runs_before
    assert stages_after == stages_before
    assert snapshot_after == snapshot_before
    assert snapshot_before > 0, "the baseline run committed nothing, so the check is vacuous"
    assert stage_records(refused.stderr) == []
    assert refused.stdout == "", "a refused run emitted command output"


def test_allow_escalate_promotes_to_full_mode(
    initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    """AC2: the same drift exits 0, is recorded `mode='full'`, and runs the full chain."""
    baseline = er_run_all()
    assert baseline.returncode == 0, baseline.stdout + baseline.stderr
    first_run_id = run_id_of(baseline)

    edited = drifted_config(tmp_path, auto_merge=0.96)
    escalated = er_run_all("--allow-escalate", config=edited)

    assert escalated.returncode == 0, escalated.stdout + escalated.stderr
    assert "--allow-escalate" in escalated.stderr
    escalated_run_id = run_id_of(escalated)
    assert escalated_run_id != first_run_id, "the escalated run reused the baseline's run_id"

    with connect() as connection:
        runs = query(
            connection,
            f"SELECT mode, status FROM {SCHEMA_QUALIFIER}.runs WHERE run_id = ?",
            escalated_run_id,
        )
        stages = query(
            connection,
            f"SELECT stage FROM {SCHEMA_QUALIFIER}.run_stages WHERE run_id = ? ORDER BY seq",
            escalated_run_id,
        )
    # "promotes the run to `--mode full`": recorded as a full run, not as an
    # incremental one that happened to execute a full chain.
    assert runs == [(MODE_FULL, "succeeded")]
    assert [stage for (stage,) in stages] == list(CHAIN_STAGES)

    # `run_stages` records a stage's NAME and never its flags (S5), and both chains
    # execute the same four stages — so the flags are asserted against the one function
    # that builds a chain, which is what the CLI called with the guard's mode.
    executed = {stage.name: stage.args for stage in run_all_chain(MODE_FULL, True)}
    assert executed == {
        "standardize": (),
        "match": ("--mode", MODE_FULL),
        "reconcile": (),
        "assemble": (),
    }
    # Not vacuous: the mode the operator typed would have run a different chain.
    refused_chain = {stage.name: stage.args for stage in run_all_chain(MODE_INCREMENTAL, True)}
    assert refused_chain["standardize"] == ("--changed-only",)
    assert refused_chain["assemble"] == ("--touched-only",)


def test_first_run_and_failed_prior_runs_do_not_trip_the_guard(
    initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    """AC4: an empty `runs` table is no baseline, and neither is a failed run."""
    edited = drifted_config(tmp_path, auto_merge=0.96)
    assert last_successful_run(initialised_lake, TENANT) is None

    first = er_run_all(config=edited)
    assert first.returncode == 0, first.stdout + first.stderr

    # Back to an empty ledger, so the second arm is about the failed row and not about
    # the run the first arm just succeeded with.
    initialised_lake.execute(f"DELETE FROM {SCHEMA_QUALIFIER}.run_stages")
    initialised_lake.execute(f"DELETE FROM {SCHEMA_QUALIFIER}.runs")
    failed_run_id = seed_prior_run(initialised_lake, status_succeeded=False)

    with connect() as connection:
        assert query(
            connection,
            f"SELECT status FROM {SCHEMA_QUALIFIER}.runs WHERE run_id = ?",
            failed_run_id,
        ) == [("failed",)]
        # The seeded row differs in every field, so a guard that read it would refuse.
        assert last_successful_run(connection, TENANT) is None

    after_failure = er_run_all(config=edited)
    assert after_failure.returncode == 0, after_failure.stdout + after_failure.stderr


def test_full_mode_never_consults_the_guard(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC5: all four fields drifted, and `--mode full` still exits 0."""
    seed_prior_run(initialised_lake, status_succeeded=True)
    prior = last_successful_run(initialised_lake, TENANT)
    assert prior is not None
    assert prior.config_hash == SEEDED_CONFIG_HASH
    assert prior.model_version == SEEDED_MODEL_VERSION
    assert prior.std_version == SEEDED_STD_VERSION
    assert prior.survivorship_version == SEEDED_SURVIVORSHIP_VERSION

    # The control, and it runs first: it writes nothing, so the full run below still
    # meets the seeded baseline rather than one this test just succeeded with.
    refused = er_run_all()
    assert refused.returncode == 3, refused.stdout + refused.stderr
    for field in ("config_hash", "model_version", "std_version", "survivorship_version"):
        assert field in refused.stderr, f"{field} drifted and was not named"

    full = er_run_all(mode=MODE_FULL)
    assert full.returncode == 0, full.stdout + full.stderr
    with connect() as connection:
        assert query(
            connection,
            f"SELECT mode, status FROM {SCHEMA_QUALIFIER}.runs WHERE run_id = ?",
            run_id_of(full),
        ) == [(MODE_FULL, "succeeded")]
