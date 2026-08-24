"""T-ASSERT-2: CONTRADICTION-1 fails the run, and writes nothing (S4.4.1, M6).

An `always` closure that contains a `never` pair is an assertion set no clustering can
honour: the two records must be together and must not be. S4.4.1 makes that a **hard,
deterministic, pre-clustering failure** — exit ``1``, `error_class='contradiction'`,
zero events, zero membership writes — and M6 is explicit that it is never a warning and
never a best-effort resolution. A pipeline that quietly picked one of the two
constraints would silently discard a steward's decision, and the steward would have no
way to tell which.

**The no-write claim is asserted, not assumed.** The ticket is explicit that a failed
run must leave `entity_membership` byte-identical. This module snapshots the relation's
full contents before the run and compares afterwards rather than trusting DuckLake to
roll back — the guarantee comes from CONTRADICTION-1 running before clustering and
before any write, and an assertion that only checked the exit code would still pass if
that ordering were lost.

**Node id.** S8.3 lists T-ASSERT-2 at
`tests/integration/test_assertions.py::test_contradiction_1_fails_the_run` while the
board realises it here. The ticket resolves the divergence: keep the **function name**
identical so the S8.3 row still resolves, and do not create a second copy in
`test_assertions.py`. Both are honoured — `test_contradiction_1_fails_the_run` below is
the only implementation of that row.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.invariants import assert_membership_equals_components
from helpers.model import fixture_settings, load_fixture_model
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.errors import ErrorClass, ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.full import score_full
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.obs.counters import DECLARED_COUNTERS, StageCounters
from er.obs.runctx import StageRun

SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"

RECONCILE_STAGE: Final = "reconcile"
MATCH_STAGE: Final = "match"

#: Who the contradiction is attributed to in `assertions.created_by`.
STEWARD: Final = "t-assert-2"


def config() -> Config:
    """The validated S6 document this session runs against (S7.1)."""
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    """The dbt var payload for one config, from S4.2's one generator."""
    payload, _ = blocking_rules_from_config(cfg)
    return payload


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def deliver(scenario: Scenario, root: Path) -> Path:
    """Materialise the base phase as the drop-folder root `er ingest --path` reads."""
    for source, path in scenario.inputs_for(PHASE).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def membership_rows(connection: duckdb.DuckDBPyConnection) -> list[tuple[Any, ...]]:
    """Every membership row in a stable order — the byte-identity snapshot of AC3."""
    return connection.execute(
        f"SELECT source_system, source_record_id, record_key, entity_id, assigned_at, run_id "
        f"FROM {MEMBERSHIP} ORDER BY source_system, source_record_id"
    ).fetchall()


@dataclass
class Dbt:
    """dbt as a stage invokes it: real `--vars`, no connection spanning it (S4.0b)."""

    connection: duckdb.DuckDBPyConnection
    artifacts: Path
    cfg: Config

    def __call__(self, command: str, select: str | None = None) -> DbtResult:
        return run_dbt(
            command,
            select=select,
            vars=render_dbt_vars(
                self.cfg, str(ULID()), extra={BLOCKING_DBT_VAR: blocking_payload(self.cfg)}
            ),
            target="lake",
            close_conn=lambda: detach(self.connection),
            reopen_conn=self._reattach,
            project_dir=DBT_PROJECT_DIR,
            profiles_dir=DBT_PROFILES_DIR,
            artifacts_dir=self.artifacts,
        )

    def standardize(self) -> None:
        """The selection `er standardize` runs: staging, then intermediate (S4.2)."""
        self("build", select=STAGING_SELECTOR)
        self("build", select=INTERMEDIATE_SELECTOR)

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


@dataclass(frozen=True)
class Reconciled:
    """A lake that has been scored and reconciled once, with three records to assert on."""

    connection: duckdb.DuckDBPyConnection
    triple: tuple[str, str, str]


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return config()


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored."""
    if (Path(DBT_PROJECT_DIR) / "dbt_packages" / "dbt_utils").is_dir():
        return
    completed = subprocess.run(
        ["dbt", "deps", "--project-dir", DBT_PROJECT_DIR, "--profiles-dir", DBT_PROFILES_DIR],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


@pytest.fixture
def reconciled(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[Reconciled]:
    """`base_10` scored and reconciled once, so there is membership to leave unchanged.

    The contradiction is asserted against a lake that already holds a partition. A
    contradiction on an empty lake would satisfy "membership unchanged" trivially, and
    the claim worth making is that a run which *would* have rewritten membership does
    not get the chance.
    """
    scenario = load_scenario(SCENARIO_NAME)
    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")
    delivery = deliver(scenario, tmp_path / "drop")
    # One run for the delivery and the reconcile that consumes it: S4.5.1's batch arm
    # is scoped by `run_id` through `ingest_batches`, which is why `er run-all` chains
    # the two under a single run.
    ingest_run = str(ULID())
    for source in scenario.inputs_for(PHASE):
        result = run_er(
            "ingest", "--source", source, "--path", str(delivery), "--run-id", ingest_run
        )
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
    dbt.standardize()

    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        model_version, tf_snapshot_id, _ = load_fixture_model(initialised_lake)
        score_full(
            initialised_lake,
            cfg,
            StageRun(
                run_id=str(ULID()),
                stage=MATCH_STAGE,
                seq=1,
                started_at=datetime.now(UTC),
                counters=StageCounters(DECLARED_COUNTERS[MATCH_STAGE]),
            ),
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            settings=fixture_settings(),
        )
        first = run_er("reconcile", "--run-id", ingest_run)
        assert first.returncode == int(ExitCode.SUCCESS), first.stdout + first.stderr

        keys = [
            str(row[0])
            for row in initialised_lake.execute(
                f"SELECT record_key FROM {STD_RECORDS} ORDER BY record_key LIMIT 3"
            ).fetchall()
        ]
        assert len(keys) == 3, "base_10 has fewer than three records; the triple needs three"
        yield Reconciled(connection=initialised_lake, triple=(keys[0], keys[1], keys[2]))
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


def _assert_triple(connection: duckdb.DuckDBPyConnection, triple: tuple[str, str, str]) -> None:
    """Write `always(a,b)`, `always(b,c)` and `never(a,c)` — S4.4.1's exact shape.

    `a` and `c` sit in one always-closure component through `b`, and the `never`
    forbids the pair the closure requires. Written through `er assert` rather than by
    INSERT so the assertions are exactly what a steward's command produces, including
    the canonicalisation S5.0 applies on the way in.
    """
    first, second, third = triple
    for kind, left, right in (
        ("always", first, second),
        ("always", second, third),
        ("never", first, third),
    ):
        result = run_er("assert", "add", "--kind", kind, "--a", left, "--b", right, "--by", STEWARD)
        assert result.returncode == int(ExitCode.SUCCESS), (
            f"er assert add {kind} {left} {right} failed:\n{result.stdout}\n{result.stderr}"
        )


def test_contradiction_1_fails_the_run(reconciled: Reconciled) -> None:
    """T-ASSERT-2 / AC3: exit 1, classified, with all three ids and the closure named."""
    _assert_triple(reconciled.connection, reconciled.triple)

    run_id = str(ULID())
    refused = run_er("reconcile", "--run-id", run_id)
    assert refused.returncode == int(ExitCode.STAGE_FAILURE), (
        f"exit {refused.returncode}, expected {int(ExitCode.STAGE_FAILURE)}; "
        f"CONTRADICTION-1 is a hard failure and never a warning (S4.4.1, M6).\n"
        f"{refused.stdout}\n{refused.stderr}"
    )

    rows = reconciled.connection.execute(
        f"SELECT error_class, error_detail FROM {RUN_STAGES} WHERE run_id = ? AND stage = ?",
        [run_id, RECONCILE_STAGE],
    ).fetchall()
    assert len(rows) == 1, f"{len(rows)} run_stages rows for the refused run; S5.2 writes one"
    error_class, error_detail = rows[0]
    assert str(error_class) == str(ErrorClass.CONTRADICTION), (
        f"error_class is {error_class!r}, not {str(ErrorClass.CONTRADICTION)!r} (S4.7)"
    )

    detail = str(error_detail)
    identifiers = [
        str(row[0])
        for row in reconciled.connection.execute(
            f"SELECT assertion_id FROM {SCHEMA_QUALIFIER}.assertions "
            "WHERE active AND created_by = ?",
            [STEWARD],
        ).fetchall()
    ]
    assert len(identifiers) == 3, f"expected three active assertions, found {len(identifiers)}"
    for identifier in identifiers:
        assert identifier in detail, (
            f"error_detail does not name assertion {identifier!r}; the operator has to "
            f"retract one of the three and cannot choose without seeing them all.\n{detail}"
        )
    for record in reconciled.triple:
        assert record in detail, (
            f"error_detail does not name {record!r} from the always-closure component; "
            f"the component is what makes the contradiction legible.\n{detail}"
        )


def test_contradiction_leaves_membership_unchanged(reconciled: Reconciled) -> None:
    """AC3/AC7: the refused run writes nothing — asserted, not assumed."""
    before = membership_rows(reconciled.connection)
    events_before = int(scalar(reconciled.connection, f"SELECT count(*) FROM {EVENTS}"))
    assert before, "no membership to preserve; the assertion below would be vacuous"

    _assert_triple(reconciled.connection, reconciled.triple)
    refused = run_er("reconcile")
    assert refused.returncode == int(ExitCode.STAGE_FAILURE), refused.stdout + refused.stderr

    assert membership_rows(reconciled.connection) == before, (
        "the refused run changed entity_membership. CONTRADICTION-1 runs before "
        "clustering and before any write, so a failed run leaves the relation "
        "byte-identical (S4.4.1, S4.7) — this is not a rollback claim"
    )
    assert int(scalar(reconciled.connection, f"SELECT count(*) FROM {EVENTS}")) == events_before, (
        "the refused run emitted an event; S4.4.1 requires zero"
    )

    # And the lake is still coherent afterwards, which is what lets the next run
    # proceed once a steward retracts one of the three.
    assert_membership_equals_components(reconciled.connection)
