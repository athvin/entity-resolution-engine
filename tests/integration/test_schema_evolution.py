"""S5.1 end to end against a live namespace (DesignDoc.md S4.0, S4.7, S5, S5.1, S8.1).

The classification itself is pure and its table is `tests/unit/test_schema_evolution.py`'s
(AC8). What needs a lake, and is therefore here, is everything the classification is
*about*:

* that a second `er init` against an unchanged lake issues no DDL at all — not "a
  statement that happens to be a no-op", but nothing;
* that an additive difference becomes exactly one `ALTER TABLE … ADD COLUMN` and leaves
  the rows that predate it NULL in the new column;
* that a breaking difference aborts `er init` **and** a stage's preflight with exit `3`
  and the literal `ERR_SCHEMA_BREAKING` message, with the lake's snapshot version
  unchanged across the failed invocation — S5.1's "no snapshot is committed" is a claim
  about a number, and this is where the number is read;
* that the *same* drifted lake exits `1` from `er doctor` and `3` from `er init`, which
  is the one place the two codes can be compared and is why AC4 asks for it in one test;
* that a `std_version` or `survivorship_version` bump records `runs.rebuild_reason` and
  runs the affected stages non-incrementally, and that such a run is outside T-INC-2's
  accounting;
* and that a snapshot taken before an additive `ALTER` still reads back, with an
  explicit projection and a reference version read from `run_stages` at runtime.

Every command is a real subprocess, for the reason `tests/integration/test_init.py`
gives: the exit code is a property of the process, and calling the function in-process
would leave the wiring — the verb, its status, its stderr — untested. The two places
that do call the library directly are the ones asserting something no CLI surface
exposes: the *statements* `apply`/`evolve` executed, and the `run_stages` row the time
travel reads its reference version from.

The drifted relation is `cut_edges` on purpose. Nothing in the S5.2 bookkeeping writes
to it, so "no `run_stages` row was written" is an assertion about the preflight rather
than about an INSERT that would have failed anyway.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Mapping
from pathlib import Path
from typing import Any

import duckdb
import yaml
from ulid import ULID

from er.cli import run_all_chain
from er.lake.ddl import apply, evolve, live_columns
from er.lake.ducklake import connect, current_snapshot
from er.lake.model import DDL_OWNED, REGISTRY, SCHEMA_QUALIFIER
from er.obs.runctx import RunContext, held
from er.versions import (
    MODE_FULL,
    MODE_INCREMENTAL,
    REBUILD_STD_VERSION_BUMP,
    REBUILD_SURVIVORSHIP_VERSION_BUMP,
    run_is_inc_accountable,
)

#: `configs/test.yaml`'s tenant, which Compose supplies as `ER_CONFIG` (S7.1). A
#: literal, so a wrong value in the document is a failure rather than a tautology.
TENANT = "test"

#: `runs.mode` for a standalone stage invocation (S5); the time-travel case records
#: one of its own so the reference snapshot has a `(run_id, stage)` to be read from.
MODE_STAGE = "stage"

#: The four stages `er run-all --skip-ingest` chains, in order (S4.0).
CHAIN_STAGES = ("standardize", "match", "reconcile", "assemble")

# The breaking difference every AC3/AC4 case is built from. `cut_edges` is untouched
# by the S5.2 bookkeeping, and `match_probability` is `DOUBLE` in S5 — so a live
# `VARCHAR` is the narrowed type S5.1 refuses to migrate.
DRIFT_RELATION = "cut_edges"
DRIFT_COLUMN = "match_probability"
DRIFT_LIVE_TYPE = "VARCHAR"
DRIFT_DECLARED_TYPE = "DOUBLE"

# S5.1's message, character for character. Spelled out rather than imported from the
# module under test: a test that reused the implementation's own constant could not
# catch a wrong constant, and AC3 is precisely a claim about one exact string.
ERR_SCHEMA_BREAKING_LINE = (
    f"ERR_SCHEMA_BREAKING: {DRIFT_RELATION}.{DRIFT_COLUMN}: "
    f"{DRIFT_LIVE_TYPE} -> {DRIFT_DECLARED_TYPE}"
)

#: The `er doctor` row that difference must falsify, and only it.
DRIFT_CHECK = f"schema drift: {DRIFT_RELATION}"

# The additive difference AC1 and AC7 are built from: `raw_records.deleted_at` is
# nullable in S5 and is the M15 deletion column, so a lake built before it is exactly
# what "a nullable column was added to the registry" leaves behind.
ADDITIVE_RELATION = "raw_records"
ADDITIVE_COLUMN = "deleted_at"
ADDITIVE_TYPE = "TIMESTAMP"

#: One `raw_records` row in the pre-`deleted_at` shape — seven values, in S5 order.
PRE_ALTER_ROW = "('crm', '1', '{}', 'h0', false, 'B1', now())"

#: The fields a seeded `runs` row needs that no document supplies here.
SEEDED_CONFIG_HASH = "f" * 64
SEEDED_STD_VERSION = "1"
SEEDED_SURVIVORSHIP_VERSION = "1"


def run_er(*args: str, config: Path | None = None) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    arguments = list(args)
    if config is not None:
        arguments += ["--config", str(config)]
    return subprocess.run(
        ["er", *arguments], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def er_run_all(
    *extra: str, config: Path | None = None, mode: str
) -> subprocess.CompletedProcess[str]:
    """`er run-all --mode <mode> --skip-ingest`, the invocation the AC5 case makes."""
    return run_er("run-all", "--mode", mode, "--skip-ingest", *extra, config=config)


def actions(stdout: str) -> list[tuple[str, str]]:
    """`er init`'s human stdout as ``(relation, action)`` pairs, in emission order."""
    pairs: list[tuple[str, str]] = []
    for line in stdout.splitlines():
        relation, _, action = line.partition(" ")
        pairs.append((relation, action))
    return pairs


def check_records(stdout: str) -> list[dict[str, Any]]:
    """`er doctor --json`'s check objects, in print order."""
    return [json.loads(line) for line in stdout.splitlines() if line.strip()]


def stage_records(stderr: str) -> list[dict[str, Any]]:
    """The S5.2 records on stderr, in emission order."""
    return [json.loads(line) for line in stderr.splitlines() if line.startswith("{")]


def run_id_of(result: subprocess.CompletedProcess[str]) -> str:
    """The one `run_id` an `er run-all` threaded through every stage (S4.0)."""
    identifiers = {record["run_id"] for record in stage_records(result.stderr)}
    assert len(identifiers) == 1, f"expected one run_id, got {sorted(identifiers)}"
    return str(identifiers.pop())


def live_shapes() -> Mapping[str, Mapping[str, str]]:
    """Every `ddl.py`-owned relation's live columns, on a connection of their own.

    A fresh connection because the shapes under test are changed by another process; a
    handle this session already holds would answer with the catalog it last read, and
    "the columns are identical" would be true of a cache rather than of the lake.
    """
    with connect() as connection:
        return {relation: dict(live_columns(connection, relation)) for relation in DDL_OWNED}


def ledger() -> tuple[list[Any], list[Any], int]:
    """`(runs rows, run_stages rows, snapshot version)`, on a connection of their own."""
    with connect() as connection:
        return (
            query(
                connection,
                f"SELECT run_id, mode, status FROM {SCHEMA_QUALIFIER}.runs ORDER BY run_id",
            ),
            query(
                connection,
                f"SELECT run_id, stage, status FROM {SCHEMA_QUALIFIER}.run_stages "
                f"ORDER BY run_id, seq",
            ),
            current_snapshot(connection),
        )


def query(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: Any) -> list[Any]:
    return connection.execute(sql, list(parameters)).fetchall()


def recreate(
    connection: duckdb.DuckDBPyConnection,
    relation: str,
    *,
    without: str = "",
    retyped: Mapping[str, str] | None = None,
) -> None:
    """Rebuild ``relation`` from its registry spec, minus or re-typing one column.

    The lake state a *previous revision of the registry* would have left, built out of
    the current registry with exactly one mutation — so the only way the live relation
    differs from S5 is the one the calling test is about. Nullability is preserved for
    the same reason.
    """
    types = dict(retyped or {})
    columns = [
        f"{column.name} {types.get(column.name, column.type)}"
        f"{'' if column.nullable else ' NOT NULL'}"
        for column in REGISTRY[relation].columns
        if column.name != without
    ]
    connection.execute(f"DROP TABLE {SCHEMA_QUALIFIER}.{relation}")
    connection.execute(f"CREATE TABLE {SCHEMA_QUALIFIER}.{relation} ({', '.join(columns)})")


def edited_config(tmp_path: Path, name: str, **versions: str) -> Path:
    """The repo's own document with `versions:` edited, where the CLI can read it.

    S5.1's bump is an edit to `versions.std_version` or `versions.survivorship_version`
    and to nothing else, so what the run sees is that field plus the `config_hash` it
    necessarily moves — and never a second difference the guard might have reacted to.
    """
    document = yaml.safe_load(Path(os.environ["ER_CONFIG"]).read_text(encoding="utf-8"))
    document["versions"].update(versions)
    edited = tmp_path / name
    edited.write_text(yaml.safe_dump(document, sort_keys=False), encoding="utf-8")
    return edited


def rebuild_reason(connection: duckdb.DuckDBPyConnection, run_id: str) -> str | None:
    row = connection.execute(
        f"SELECT rebuild_reason FROM {SCHEMA_QUALIFIER}.runs WHERE run_id = ?", [run_id]
    ).fetchone()
    assert row is not None, f"no runs row for {run_id}"
    return None if row[0] is None else str(row[0])


def reference_snapshot(connection: duckdb.DuckDBPyConnection, run_id: str, stage: str) -> int:
    """The version a `(run_id, stage)` committed up to, read at runtime (S8.1).

    S8.1's blanket rule: "every snapshot-dependent test MUST capture its reference
    snapshot id at runtime from `run_stages.snapshot_start` / `run_stages.snapshot_end`
    for a named `(run_id, stage)`, and no test may reference an absolute snapshot
    version". This is that read, and it is the only source of the version below.
    """
    row = connection.execute(
        f"SELECT snapshot_end FROM {SCHEMA_QUALIFIER}.run_stages WHERE run_id = ? AND stage = ?",
        [run_id, stage],
    ).fetchone()
    assert row is not None and row[0] is not None, f"no recorded range for ({run_id}, {stage})"
    return int(row[0])


def test_init_is_idempotent_and_issues_no_ddl(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC2: two `er init`s on an unchanged lake, both exit 0, and nothing is executed."""
    before = live_shapes()

    first = run_er("init")
    second = run_er("init")

    assert first.returncode == 0, first.stdout + first.stderr
    assert second.returncode == 0, second.stdout + second.stderr
    assert actions(second.stdout) == [(relation, "exists") for relation in DDL_OWNED]
    assert live_shapes() == before

    # "issues zero DDL statements" in its strongest form: `apply` reports a statement
    # for every relation it created and `None` for one it found, and `evolve` returns
    # the statements it ran. Both are empty of statements against an unchanged lake,
    # which is what makes the S5.1 no-op a no-op rather than a harmless re-CREATE.
    assert [result.statement for result in apply(initialised_lake)] == [None] * len(DDL_OWNED)
    assert evolve(initialised_lake) == ()
    assert live_shapes() == before


def test_additive_alter_applies_and_backfills_null(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC1: `er init` adds the one missing nullable column and leaves old rows NULL."""
    recreate(initialised_lake, ADDITIVE_RELATION, without=ADDITIVE_COLUMN)
    initialised_lake.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.{ADDITIVE_RELATION} VALUES {PRE_ALTER_ROW}"
    )
    before = live_shapes()
    assert ADDITIVE_COLUMN not in before[ADDITIVE_RELATION]

    result = run_er("init")

    assert result.returncode == 0, result.stdout + result.stderr
    after = live_shapes()
    # Exactly one column added, to exactly one relation, and every other column of
    # every relation byte-identical: that is "exactly one ALTER TABLE … ADD COLUMN"
    # stated in terms the command's own surface can be held to.
    assert set(after[ADDITIVE_RELATION]) - set(before[ADDITIVE_RELATION]) == {ADDITIVE_COLUMN}
    assert after[ADDITIVE_RELATION][ADDITIVE_COLUMN] == ADDITIVE_TYPE
    assert {
        relation: {name: type_ for name, type_ in columns.items() if name != ADDITIVE_COLUMN}
        for relation, columns in after.items()
    } == dict(before)

    with connect() as connection:
        rows = query(
            connection, f"SELECT {ADDITIVE_COLUMN} FROM {SCHEMA_QUALIFIER}.{ADDITIVE_RELATION}"
        )
    assert rows == [(None,)], "the row that predates the ALTER is not NULL in the new column"


def test_breaking_change_exits_3_with_named_message_and_no_snapshot(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC3: `er init` and a stage preflight both exit 3, and nothing is committed."""
    recreate(initialised_lake, DRIFT_RELATION, retyped={DRIFT_COLUMN: DRIFT_LIVE_TYPE})
    runs_before, stages_before, snapshot_before = ledger()
    assert snapshot_before > 0, "the namespace has committed nothing, so the check is vacuous"

    refused_init = run_er("init")

    assert refused_init.returncode == 3, refused_init.stdout + refused_init.stderr
    assert ERR_SCHEMA_BREAKING_LINE in refused_init.stderr.splitlines(), (
        f"stderr carries no line equal to the S5.1 message:\n{refused_init.stderr}"
    )
    assert refused_init.stdout == "", "a refused init reported relations it did not touch"

    # The same registry state refuses a STAGE, before it writes its `run_stages` row.
    refused_stage = run_er("standardize")

    assert refused_stage.returncode == 3, refused_stage.stdout + refused_stage.stderr
    assert ERR_SCHEMA_BREAKING_LINE in refused_stage.stderr.splitlines()
    assert stage_records(refused_stage.stderr) == [], "a refused stage emitted a stage record"
    assert refused_stage.stdout == ""

    # S5.1's "No snapshot is committed", read as the number it is: the ledger and the
    # lake's version are what they were before either invocation.
    assert ledger() == (runs_before, stages_before, snapshot_before)


def test_doctor_reports_drift_and_exits_1(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC4: the same drift is `1` from the doctor and `3` from `er init` — one test.

    The two codes are asserted distinct here because that is the only place they can
    be compared. A doctor check failure is a check failure under the S4.0 exit-code
    table, while the same difference met by `er init` or a stage preflight is an S4.7
    `precondition`; asserting each in its own file would leave the pair untested.
    """
    healthy = run_er("doctor")
    assert healthy.returncode == 0, healthy.stdout + healthy.stderr

    recreate(initialised_lake, DRIFT_RELATION, retyped={DRIFT_COLUMN: DRIFT_LIVE_TYPE})
    drifted = run_er("doctor", "--json")

    assert drifted.returncode == 1, drifted.stdout + drifted.stderr
    failures = [record for record in check_records(drifted.stdout) if record["verdict"] == "fail"]
    assert [record["check"] for record in failures] == [DRIFT_CHECK], (
        f"the drift failed more than its own row: {failures}"
    )
    # "naming the relation and column": the row carries the S5.1 message, so an
    # operator reading CI output knows what to migrate without a second run.
    assert DRIFT_RELATION in failures[0]["actual"]
    assert DRIFT_COLUMN in failures[0]["actual"]

    refused = run_er("init")

    assert refused.returncode == 3, refused.stdout + refused.stderr
    assert drifted.returncode == 1
    assert drifted.returncode != refused.returncode


def test_version_bump_sets_rebuild_reason_and_runs_non_incrementally(
    initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    """AC5/AC6: each bump names itself in `runs.rebuild_reason` and leaves T-INC-2."""
    baseline = er_run_all(mode=MODE_INCREMENTAL)
    assert baseline.returncode == 0, baseline.stdout + baseline.stderr
    baseline_run_id = run_id_of(baseline)

    # A `std_version` bump is drift the S4.0 guard catches, and `--allow-escalate`
    # promotes it to the full chain S5.1 requires after one (S5.1, S4.0).
    std_bumped = edited_config(tmp_path, "std_bump.yaml", std_version="2")
    escalated = er_run_all("--allow-escalate", config=std_bumped, mode=MODE_INCREMENTAL)
    assert escalated.returncode == 0, escalated.stdout + escalated.stderr
    std_run_id = run_id_of(escalated)

    # A `survivorship_version` bump against the run above, so `std_version` matches
    # its baseline and the survivorship half is the only thing that moved. `--mode
    # full` is never guarded, which is why this arm needs no `--allow-escalate` and
    # still has to record its reason: S5.1 ties the reason to the bump, not to how
    # the run reached full mode.
    survivorship_bumped = edited_config(
        tmp_path, "survivorship_bump.yaml", std_version="2", survivorship_version="2"
    )
    survivorship = er_run_all(config=survivorship_bumped, mode=MODE_FULL)
    assert survivorship.returncode == 0, survivorship.stdout + survivorship.stderr
    survivorship_run_id = run_id_of(survivorship)

    with connect() as connection:
        assert rebuild_reason(connection, baseline_run_id) is None
        assert rebuild_reason(connection, std_run_id) == REBUILD_STD_VERSION_BUMP
        assert rebuild_reason(connection, survivorship_run_id) == (
            REBUILD_SURVIVORSHIP_VERSION_BUMP
        )
        assert query(
            connection,
            f"SELECT mode FROM {SCHEMA_QUALIFIER}.runs WHERE run_id IN (?, ?) ORDER BY run_id",
            std_run_id,
            survivorship_run_id,
        ) == [(MODE_FULL,), (MODE_FULL,)]

        # AC6: a planned rebuild is outside T-INC-2's accounting and an ordinary run
        # is inside it. Both directions, because a predicate that answered False for
        # everything would satisfy the first half alone.
        assert run_is_inc_accountable(connection, baseline_run_id) is True
        assert run_is_inc_accountable(connection, std_run_id) is False
        assert run_is_inc_accountable(connection, survivorship_run_id) is False

    # `run_stages` records a stage's NAME and never its flags (S5), so "runs the
    # affected stages non-incrementally" is asserted against the one function that
    # builds a chain — called with the mode both bumped runs were recorded under.
    executed = {stage.name: stage.args for stage in run_all_chain(MODE_FULL, True)}
    assert executed["standardize"] == (), "a std_version bump ran --changed-only"
    assert executed["assemble"] == (), "a survivorship_version bump ran --touched-only"
    assert set(executed) == set(CHAIN_STAGES)

    # Not vacuous: the incremental chain the guard refused carries both flags.
    refused_chain = {stage.name: stage.args for stage in run_all_chain(MODE_INCREMENTAL, True)}
    assert refused_chain["standardize"] == ("--changed-only",)
    assert refused_chain["assemble"] == ("--touched-only",)


def test_time_travel_across_an_additive_change(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC7: a pre-ALTER snapshot reads back under an explicit projection (S5.1, S8.1).

    S5.1 supports time travel across additive changes only, and says why the
    projection matters: "a snapshot taken before an `ADD COLUMN` reads back with the
    column absent; a query written against the current schema MUST therefore project
    explicitly rather than `SELECT *`". So the projection here is the *pre-ALTER*
    column list, captured before the column existed, and the reference version is read
    from `run_stages` — no absolute snapshot version appears anywhere in this file.
    """
    recreate(initialised_lake, ADDITIVE_RELATION, without=ADDITIVE_COLUMN)
    pre_alter_columns = tuple(live_columns(initialised_lake, ADDITIVE_RELATION))
    assert ADDITIVE_COLUMN not in pre_alter_columns

    run_id = str(ULID())
    context = RunContext(
        run_id=run_id,
        mode=MODE_STAGE,
        tenant=TENANT,
        config_hash=SEEDED_CONFIG_HASH,
        std_version=SEEDED_STD_VERSION,
        survivorship_version=SEEDED_SURVIVORSHIP_VERSION,
        source=held(initialised_lake),
    )
    # Driven through the one writer of `run_stages` (S5.2) rather than INSERTed: the
    # snapshot range this test reads has to be the range that writer records, or the
    # reference version would be a number this test invented.
    with context as run, run.stage("ingest") as stage_run:
        initialised_lake.execute(
            f"INSERT INTO {SCHEMA_QUALIFIER}.{ADDITIVE_RELATION} VALUES {PRE_ALTER_ROW}"
        )
        stage_run.finish(0)

    statements = evolve(initialised_lake)

    assert len(statements) == 1, statements
    assert f"ADD COLUMN {ADDITIVE_COLUMN} {ADDITIVE_TYPE}" in statements[0]
    assert ADDITIVE_COLUMN in live_columns(initialised_lake, ADDITIVE_RELATION)

    snapshot = reference_snapshot(initialised_lake, run_id, "ingest")
    historical = initialised_lake.execute(
        f"SELECT {', '.join(pre_alter_columns)} "
        f"FROM {SCHEMA_QUALIFIER}.{ADDITIVE_RELATION} AT (VERSION => {snapshot})"
    ).fetchall()

    assert len(historical) == 1
    assert historical[0][:2] == ("crm", "1")
    assert len(historical[0]) == len(pre_alter_columns)

    # And the same row read at the current version carries the added column, NULL.
    assert query(
        initialised_lake,
        f"SELECT {ADDITIVE_COLUMN} FROM {SCHEMA_QUALIFIER}.{ADDITIVE_RELATION}",
    ) == [(None,)]


def test_a_dbt_owned_relation_is_never_altered(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """This ticket issues no DDL against a dbt-owned relation (S5.0, out of scope).

    A dbt-owned relation with a column set that is wrong in both directions must
    survive `er init` byte-identical: it evolves through its enforced contract, and a
    `ddl.py` preflight that refused over one would make every drifted mart abort the
    pipeline instead of failing `dbt build` with exit `1` (S5.1).
    """
    initialised_lake.execute(
        f"CREATE TABLE {SCHEMA_QUALIFIER}.int_std_records (record_key VARCHAR, bogus_column BIGINT)"
    )
    try:
        result = run_er("init")

        assert result.returncode == 0, result.stdout + result.stderr
        assert "int_std_records" not in [relation for relation, _ in actions(result.stdout)]
        with connect() as connection:
            assert dict(live_columns(connection, "int_std_records")) == {
                "record_key": "VARCHAR",
                "bogus_column": "BIGINT",
            }
    finally:
        initialised_lake.execute(f"DROP TABLE {SCHEMA_QUALIFIER}.int_std_records")
