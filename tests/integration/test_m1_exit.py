"""The M1 exit gate: S12's exit criteria, clause by clause (DesignDoc.md S12, S4.0).

S12 is a gate — "M1 cannot exit until…" — and its M1 row is the milestone's only
falsifiable exit. Everything here is that row and nothing else: this module adds no
coverage of its own, it *executes* the criterion. A failure is fixed in the ticket
that owns the behaviour, never by weakening an assertion here.

**Each clause is quoted, not paraphrased.** :data:`M1_EXIT_CLAUSES` maps an
``M1-EXIT-<k>`` id to the literal text of that clause as it stands in the S12 M1
exit-criteria cell, and :func:`test_m1_exit_tag_parity` asserts two things about it:
every id is carried by exactly one test in this module, and every quoted string is
still a verbatim substring of that cell in `DesignDoc.md`. A paraphrase would let the
spec be edited under a green gate — the milestone would keep exiting on a criterion
nobody wrote any more — which is precisely the drift the parity arm exists to catch.

**Why the spec is readable from inside the container.** S7.3's image COPYs no
documentation, deliberately: it is later the k8s job image. So `docker/compose.yaml`
bind-mounts `DesignDoc.md` read-only into the `test`-profile `pipeline` service, which
is the only service the integration suite runs in. The spec reaches this test, and it
never becomes an image layer.

**Two namespaces, for two different preconditions.** The `er init` and `er doctor`
clauses need "a namespace with no schema", which by construction the S8.1 session
namespace is not — so they mint one of their own and reclaim it, exactly as
`tests/integration/test_init.py` does for the destructive verbs. The `er run-all`
clauses need "an empty lake", which is exactly what `initialised_lake` plus S8.1's
function isolation yields: the fourteen `ddl.py`-owned relations, created and empty.

Every invocation is a real subprocess. "`er run-all` exits **0**" is a claim about the
process, and S12 says so itself — "Stage commands that only print to stdout do not
satisfy this" — so the stage rows are read back out of the lake on a connection of
their own rather than believed from what the command printed.

Snapshot **counts** are asserted nowhere and may not be: a stage commits a range
(S4 preamble), and a no-op run may legitimately commit an empty one.
"""

from __future__ import annotations

import inspect
import json
import os
import re
import subprocess
from collections import Counter
from collections.abc import Iterator, Mapping
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from ulid import ULID

from er.doctor import RUNTIME_CHECK_NAMES
from er.lake.catalog import CatalogConnection, drop_metadata_schema, tenant_lock
from er.lake.ddl import live_columns
from er.lake.ducklake import LAKE_ALIAS, connect
from er.lake.model import DDL_OWNED, SCHEMA_QUALIFIER
from er.lake.objectstore import ObjectStore

#: The S12 M1 exit criteria, one entry per clause, each value quoted **verbatim** from
#: the exit-criteria cell of the S12 M1 row. :func:`test_m1_exit_tag_parity` is what
#: keeps them quotations rather than summaries.
M1_EXIT_CLAUSES: Final[dict[str, str]] = {
    "M1-EXIT-1": "`er init` exits 0",
    "M1-EXIT-2": "is idempotent on a second invocation",
    "M1-EXIT-3": (
        "`er doctor` exits 0 with every S2.1 row it owns plus the six T-DOCTOR-1 "
        "runtime assertions passing"
    ),
    "M1-EXIT-4": (
        "`er run-all` exits **0** — every stage returns `10` and, per S4.0, a chain "
        "of `10`s is a successful no-op run"
    ),
    "M1-EXIT-5": "writes **exactly one `runs` row** with `status='succeeded'`",
    "M1-EXIT-6": (
        "**four `run_stages` rows** (`standardize, match, reconcile, assemble`) each "
        "carrying a snapshot range"
    ),
    "M1-EXIT-7": (
        "without `--source`/`--path` the invocation is rejected with exit `2` before any stage runs"
    ),
    "M1-EXIT-8": "**no `ingest_batches` row is written** and none is asserted",
    "M1-EXIT-9": "**Zero** relations matching `__splink__%` exist in `lake`",
    "M1-EXIT-10": "a **second concurrent `er run-all` exits 3**",
}

REPO_ROOT: Final[Path] = Path(__file__).resolve().parents[2]
DESIGN_DOC: Final[Path] = REPO_ROOT / "DesignDoc.md"

#: Where the S12 milestone table lives, and where it stops. S12.1 is the decision lock,
#: which carries its own `M1` mentions, so the search is bounded by the next anchor.
S12_ANCHOR: Final[str] = '<a id="s12"></a>'

#: The S12 milestone table's M1 row. Matched on `**M1 ` rather than on the whole
#: `**M1 — Substrate**` cell so a rename of the milestone's *title* does not silently
#: match nothing and report "no M1 row" when the row is right there.
M1_ROW_RE: Final[re.Pattern[str]] = re.compile(r"^\|\s*\*\*M1\s.*$", re.MULTILINE)

#: S12's milestone table: milestone, contents, relations first written, exit criteria.
M1_ROW_CELLS: Final[int] = 4

#: An `M1-EXIT-<k>` id wherever a test's docstring carries one.
TAG_RE: Final[re.Pattern[str]] = re.compile(r"\bM1-EXIT-\d+\b")

#: The four stages `er run-all --skip-ingest` chains, in order (S4.0). S12 names
#: exactly these four `run_stages` rows, in this spelling.
CHAIN_STAGES: Final[tuple[str, ...]] = ("standardize", "match", "reconcile", "assemble")

#: S4.0's "nothing to do", as a literal. Read from :class:`~er.errors.ExitCode` it
#: would agree with the enum no matter what the enum said, and the clause is about the
#: number S4.0 writes down.
NOTHING_TO_DO: Final[int] = 10

#: The six T-DOCTOR-1 runtime assertions the M1 clause counts.
RUNTIME_CHECKS_EXPECTED: Final[int] = 6

#: `configs/test.yaml`'s tenant, which Compose supplies as `ER_CONFIG` (S7.1). A
#: literal, so a wrong value in the document is a failure rather than a tautology.
TENANT: Final[str] = "test"

#: S8.1 step 2's two spellings, for the scratch namespace the `er init` clauses need.
METADATA_SCHEMA: Final[str] = "er_test_{ns}"
DATA_PATH: Final[str] = "s3://lake/test/{ns}/"

#: What S4.0b forbids in the lake: Splink writes into `splink_scratch` in the
#: in-memory database, so anything matching this pattern was committed to DuckLake.
SPLINK_PATTERN: Final[str] = "__splink__%"


@dataclass(frozen=True)
class Namespace:
    """A namespace this module owns outright, reclaimed however the test ends."""

    ns: str
    metadata_schema: str
    data_path: str

    @property
    def env(self) -> dict[str, str]:
        """The two variables that make a child process attach this namespace (S7.2)."""
        return {
            "ER_LAKE_METADATA_SCHEMA": self.metadata_schema,
            "ER_LAKE_DATA_PATH": self.data_path,
        }


@pytest.fixture
def empty_namespace(object_store: ObjectStore, catalog: CatalogConnection) -> Iterator[Namespace]:
    """A namespace with no schema and no prefix — S12's "empty lake", before `er init`.

    Reclaimed with the two primitives `er lake reset` is built from rather than with
    the verb itself: a teardown that used the command under test would report a green
    gate for a reset that silently did nothing.
    """
    ns = str(ULID()).lower()
    namespace = Namespace(
        ns=ns,
        metadata_schema=METADATA_SCHEMA.format(ns=ns),
        data_path=DATA_PATH.format(ns=ns),
    )
    try:
        yield namespace
    finally:
        drop_metadata_schema(catalog, namespace.metadata_schema)
        object_store.delete_prefix(namespace.data_path)


def run_er(*args: str, env: Mapping[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script, in this session's namespace by default."""
    environment = dict(os.environ)
    if env is not None:
        environment.update(env)
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=environment, check=False
    )


def stdout_records(stdout: str) -> list[dict[str, Any]]:
    """The `--json` objects a command printed, in print order."""
    return [json.loads(line) for line in stdout.splitlines() if line.strip()]


@contextmanager
def attached(namespace: Namespace) -> Iterator[duckdb.DuckDBPyConnection]:
    """An S4.0b connection to ``namespace``, for asserting what the CLI left behind."""
    with pytest.MonkeyPatch.context() as patch:
        for name, value in namespace.env.items():
            patch.setenv(name, value)
        with connect() as connection:
            yield connection


def relations(connection: duckdb.DuckDBPyConnection, pattern: str | None = None) -> set[str]:
    """Every relation live in the attached lake, optionally filtered by a LIKE pattern."""
    sql = "SELECT table_name FROM duckdb_tables() WHERE database_name = ?"
    parameters: list[str] = [LAKE_ALIAS]
    if pattern is not None:
        sql += " AND table_name LIKE ?"
        parameters.append(pattern)
    return {str(name) for (name,) in connection.execute(sql, parameters).fetchall()}


def column_sets(connection: duckdb.DuckDBPyConnection) -> dict[str, dict[str, str]]:
    """Every `ddl.py`-owned relation's live columns, name to type (S5.1)."""
    return {relation: dict(live_columns(connection, relation)) for relation in DDL_OWNED}


def query(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: Any) -> list[Any]:
    return connection.execute(sql, list(parameters)).fetchall()


def count(connection: duckdb.DuckDBPyConnection, relation: str) -> int:
    row = connection.execute(f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{relation}").fetchone()
    return 0 if row is None else int(row[0])


def ledger_counts() -> tuple[int, int]:
    """`(runs, run_stages)` row counts, read on a connection of their own.

    A fresh connection because the rows under test would have been written by another
    process; a cached handle would answer with what this session last saw.
    """
    with connect() as connection:
        return count(connection, "runs"), count(connection, "run_stages")


def s12_milestone_table() -> str:
    """The S12 section of `DesignDoc.md`, up to the S12.1 anchor."""
    assert DESIGN_DOC.is_file(), (
        f"{DESIGN_DOC} is not readable. Inside the `pipeline` container the spec "
        "arrives as the read-only bind mount docker/compose.yaml declares, because "
        "S7.3's image COPYs no documentation; without it this gate cannot check its "
        "clauses against the S12 row they were quoted from."
    )
    text = DESIGN_DOC.read_text(encoding="utf-8")
    start = text.find(S12_ANCHOR)
    assert start != -1, f"DesignDoc.md has no anchor {S12_ANCHOR}"
    end = text.find('<a id="', start + 1)
    return text[start:end] if end != -1 else text[start:]


def m1_exit_criteria_cell() -> str:
    """The exit-criteria cell of the S12 M1 row — the authority for every clause here."""
    rows = M1_ROW_RE.findall(s12_milestone_table())
    assert len(rows) == 1, f"S12's milestone table has {len(rows)} M1 rows, expected exactly 1"
    cells = [cell.strip() for cell in rows[0].strip().strip("|").split("|")]
    assert len(cells) == M1_ROW_CELLS, (
        f"the S12 M1 row splits into {len(cells)} cells, not the {M1_ROW_CELLS} the "
        "milestone table declares; a '|' inside a cell would silently move the "
        "exit-criteria column"
    )
    return cells[-1]


def tagged_clause_ids() -> list[str]:
    """Every `M1-EXIT-<k>` id carried by a test docstring in this module, in name order.

    Read off the functions rather than out of the module's source text, because
    :data:`M1_EXIT_CLAUSES`' own keys are ids too: a textual scan would count every
    clause once for its declaration and call an untagged clause tagged.
    """
    found: list[str] = []
    for name, function in sorted(globals().items()):
        if not name.startswith("test_") or not inspect.isfunction(function):
            continue
        found.extend(TAG_RE.findall(inspect.getdoc(function) or ""))
    return found


def test_init_creates_ddl_owned_relations(empty_namespace: Namespace) -> None:
    """M1-EXIT-1 / AC1: on a namespace with no schema, `er init` exits 0 and creates
    exactly the `ddl.py`-owned relation set."""
    result = run_er("init", "--json", env=empty_namespace.env)

    assert result.returncode == 0, result.stdout + result.stderr
    payloads = stdout_records(result.stdout)
    assert [payload["relation"] for payload in payloads] == list(DDL_OWNED)
    assert {payload["action"] for payload in payloads} == {"created"}

    # "exactly": the set, not a superset. `er init` owns the `ddl.py` half of S5.0 and
    # nothing else, so a dbt-owned relation appearing here would mean the owner split
    # had been crossed by the one command that runs before any dbt invocation.
    with attached(empty_namespace) as connection:
        assert relations(connection) == set(DDL_OWNED)


def test_init_is_idempotent(empty_namespace: Namespace) -> None:
    """M1-EXIT-2 / AC1: a second `er init` exits 0, reports `exists` for every relation
    and changes no column set."""
    assert run_er("init", env=empty_namespace.env).returncode == 0
    with attached(empty_namespace) as connection:
        before = column_sets(connection)
    assert all(before.values()), (
        "at least one relation reported no columns, so the comparison below is vacuous"
    )

    second = run_er("init", "--json", env=empty_namespace.env)

    assert second.returncode == 0, second.stdout + second.stderr
    payloads = stdout_records(second.stdout)
    assert [payload["relation"] for payload in payloads] == list(DDL_OWNED)
    assert {payload["action"] for payload in payloads} == {"exists"}
    with attached(empty_namespace) as connection:
        assert column_sets(connection) == before


def test_doctor_exits_zero(empty_namespace: Namespace) -> None:
    """M1-EXIT-3 / AC2: `er doctor` exits 0 on that namespace with every check row
    reporting pass, the six T-DOCTOR-1 runtime assertions among them."""
    assert run_er("init", env=empty_namespace.env).returncode == 0

    result = run_er("doctor", "--json", env=empty_namespace.env)

    assert result.returncode == 0, result.stdout + result.stderr
    records = stdout_records(result.stdout)
    assert records, "`er doctor` exited 0 having printed no check rows at all"
    verdicts = {record["check"]: record["verdict"] for record in records}
    failures = sorted(name for name, verdict in verdicts.items() if verdict != "pass")
    assert not failures, f"`er doctor` exited 0 with failing rows: {failures}"

    # "plus the six T-DOCTOR-1 runtime assertions": a doctor that asserted the S2.1
    # pins and nothing else would pass on a substrate that does not work.
    assert len(RUNTIME_CHECK_NAMES) == RUNTIME_CHECKS_EXPECTED
    assert set(RUNTIME_CHECK_NAMES) <= set(verdicts)


def test_run_all_skip_ingest_exits_zero_with_four_stage_rows(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """M1-EXIT-4 / M1-EXIT-5 / M1-EXIT-6, AC3 and AC4: the chain exits 0 on a chain of
    `10`s and leaves one `runs` row and four `run_stages` rows behind it."""
    result = run_er("run-all", "--mode", "incremental", "--skip-ingest", "--json")

    assert result.returncode == 0, result.stdout + result.stderr
    payloads = stdout_records(result.stdout)
    stages = [payload for payload in payloads if "stage" in payload]
    summary = [payload for payload in payloads if "run_id" in payload]
    assert [payload["stage"] for payload in stages] == list(CHAIN_STAGES)
    assert [payload["exit_code"] for payload in stages] == [NOTHING_TO_DO] * len(CHAIN_STAGES)
    assert len(summary) == 1, "S4.0 gives the chain one final run summary"
    assert summary[0]["exit_code"] == 0

    # Read back on a connection of its own: S12 says stage commands that only print to
    # stdout do not satisfy this, so what is asserted is what reached the lake.
    with connect() as connection:
        runs = query(
            connection,
            f"SELECT run_id, status, config_hash FROM {SCHEMA_QUALIFIER}.runs",
        )
        rows = query(
            connection,
            f"SELECT stage, seq, status, snapshot_start, snapshot_end "
            f"FROM {SCHEMA_QUALIFIER}.run_stages ORDER BY seq",
        )

    assert len(runs) == 1, "S12: exactly one `runs` row"
    run_id, status, config_hash = runs[0]
    assert status == "succeeded", "S4.0: a chain of `10`s is a successful no-op run"
    assert config_hash is not None
    assert run_id == summary[0]["run_id"]

    assert [row[0] for row in rows] == list(CHAIN_STAGES), "S12: four rows, these four"
    assert [row[1] for row in rows] == [1, 2, 3, 4], "S5.2: `seq` is dense and 1-based"
    for stage, _seq, stage_status, start, end in rows:
        assert stage_status == "succeeded", f"{stage} did not succeed"
        # A range, never a count: a no-op stage commits an empty one, and asserting
        # anything about its width would make that legitimate outcome a failure.
        assert start is not None and end is not None, f"{stage} recorded no snapshot range"
        assert end >= start, f"{stage} recorded a range that runs backwards"


def test_missing_source_and_path_exits_2(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """M1-EXIT-7 / AC5: `--skip-ingest` is required, and its absence is refused before
    any stage runs."""
    before = ledger_counts()
    assert before == (0, 0), "the namespace already held run rows, so 'writes none' is vacuous"

    result = run_er("run-all", "--mode", "incremental")

    assert result.returncode == 2, result.stdout + result.stderr
    assert "--skip-ingest" in result.stderr
    # "before any stage runs": not a `runs` row, and not a `run_stages` row either --
    # the refusal happens before the RunContext is entered, so there is no ledger entry
    # for a run that never ran.
    assert ledger_counts() == before
    assert result.stdout == "", "a refused invocation emitted command output"


def test_no_ingest_batches_row_and_no_splink_relations(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """M1-EXIT-8 / M1-EXIT-9, AC6: ingest is skipped, so nothing lands in
    `ingest_batches`; and nothing Splink writes ever lands in the lake."""
    result = run_er("run-all", "--mode", "incremental", "--skip-ingest")

    assert result.returncode == 0, result.stdout + result.stderr
    with connect() as connection:
        # The run really happened -- otherwise both zeros below are free.
        assert count(connection, "runs") == 1
        assert count(connection, "ingest_batches") == 0, (
            "ingest was skipped, so no batch manifest may have been written (S12)"
        )
        assert relations(connection, SPLINK_PATTERN) == set(), (
            "S4.0b hands Splink the connection with output_schema='splink_scratch', so "
            "an intermediate in the lake means one committed a snapshot of its own"
        )


def test_second_concurrent_run_exits_3(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """M1-EXIT-10 / AC7: the second writer against one tenant is refused, and writes
    nothing (S4.0b)."""
    before = ledger_counts()
    assert before == (0, 0), "the namespace already held run rows, so 'writes none' is vacuous"

    # The overlap is created by holding the lock for the length of the second
    # invocation rather than by racing two subprocesses: the first writer's whole
    # contribution is that it holds the tenant lock, it holds it with the same
    # `tenant_lock` an `er run-all` process takes, and the second invocation is a real
    # `er run-all` that has no way to tell the difference.
    with tenant_lock(TENANT, run_id=str(ULID())):
        second = run_er("run-all", "--mode", "incremental", "--skip-ingest")

    assert second.returncode == 3, second.stdout + second.stderr
    assert ledger_counts() == before


def test_m1_exit_tag_parity() -> None:
    """The clauses are quotations, and every one of them is enforced exactly once.

    Deliberately carries no `M1-EXIT-<k>` id of its own: it is the gate on the gate,
    not one of S12's criteria.
    """
    tagged = tagged_clause_ids()
    declared = list(M1_EXIT_CLAUSES)

    # Missing, duplicated and unknown ids are all one comparison: a clause enforced
    # twice is as wrong as a clause enforced nowhere, because it makes the tag count a
    # lie about how much of S12 this module actually runs.
    assert Counter(tagged) == Counter(declared), (
        f"tags {sorted(Counter(tagged).items())} do not match the declared clause set "
        f"{sorted(declared)}"
    )

    cell = m1_exit_criteria_cell()
    drifted = [clause_id for clause_id, text in M1_EXIT_CLAUSES.items() if text not in cell]
    assert not drifted, (
        f"{drifted} are no longer verbatim in the S12 M1 exit-criteria cell. The spec "
        "moved and this gate did not: requote the clause and change the assertion that "
        "enforces it, rather than loosening either."
    )
