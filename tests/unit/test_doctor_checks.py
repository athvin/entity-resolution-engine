"""`er doctor`'s table, asserted without a substrate (DesignDoc.md S2.1, S8.1, S8.4).

Three properties belong to the unit layer because none of them needs a lake, and all
three fail silently if they are only checked inside the integration job:

* the check set IS S2.1's doctor-asserted rows plus T-DOCTOR-1's six plus S5.1's one
  drift row per `ddl.py`-owned relation, in both directions — an S2.1 row that gains
  `er doctor` in its *Asserted by* cell without gaining a probe would otherwise be a
  pin nothing asserts, and a relation added to S5 with no drift row would be a
  relation nothing compares against the registry;
* the `postgres` extension is read under its **registered** name. This is the failure
  S2.1 spells out: `duckdb_extensions()` has no row called `postgres`, so a check
  filtered on the install name matches nothing, disagrees with nothing, and passes;
* a failed check exits `1` with every *other* line still printed and still `pass` — a
  doctor that stops at the first bad row costs a second run to find the second one.

The tables under test are built through :func:`~er.doctor.build_checks`, which takes
the pin mapping as a parameter, so a wrong pin is injected without touching
:data:`~er.versions.PINS` and without patching module state a later test would inherit.
"""

from __future__ import annotations

import json
from dataclasses import replace
from typing import Any

import duckdb
import pytest
from typer.testing import CliRunner

from er.cli import app
from er.doctor import (
    CHECKS,
    RUNTIME_CHECK_NAMES,
    SCHEMA_DRIFT_CHECK_NAMES,
    SCHEMA_DRIFT_PREFIX,
    DoctorCheck,
    MissingProbeError,
    build_checks,
    check_lines,
    exit_code,
    extension_version,
    run_checks,
)
from er.lake.model import DDL_OWNED
from er.versions import EXTENSION_PINS, PINS, Pin

runner = CliRunner()

#: The doctor rows whose probe needs nothing but the metadata database — no lake, no
#: catalog, no object store, no subprocess. dbt-duckdb is excluded although it is
#: distribution-backed: S2.1 gives that row `dbt debug` as well as a version, so its
#: probe spawns dbt and belongs to the integration layer.
PURE_COMPONENTS: tuple[str, ...] = tuple(
    component
    for component, pin in PINS.items()
    if pin.asserted_by_doctor and pin.distribution is not None and component != "dbt-duckdb"
)


def pure_checks(pins: dict[str, Pin]) -> tuple[DoctorCheck, ...]:
    """The subset of ``build_checks(pins)`` that runs on a bare runner (S8.1)."""
    return tuple(check for check in build_checks(pins) if check.name in PURE_COMPONENTS)


class RecordingConnection:
    """A DuckDB connection stand-in that remembers the SQL and parameters it got."""

    def __init__(self, row: tuple[Any, ...] | None) -> None:
        self.row = row
        self.statements: list[tuple[str, Any]] = []

    def execute(self, sql: str, parameters: Any = None) -> RecordingConnection:
        self.statements.append((sql, parameters))
        return self

    def fetchone(self) -> tuple[Any, ...] | None:
        return self.row


def test_check_set_equals_s2_1_rows_plus_runtime_names() -> None:
    """AC1: the table is exactly S2.1's doctor rows, the six and S5.1's drift rows."""
    names = [check.name for check in CHECKS]
    assert len(names) == len(set(names)), f"duplicate check names: {names}"

    doctor_rows = {component for component, pin in PINS.items() if pin.asserted_by_doctor}
    behavioural = set(RUNTIME_CHECK_NAMES) | set(SCHEMA_DRIFT_CHECK_NAMES)
    assert set(names) == doctor_rows | behavioural

    # Both directions, spelled out: a row S2.1 assigns to `er doctor` with no check
    # here is a pin nothing asserts, and a check here with no S2.1 row is an
    # assertion no spec sentence backs.
    assert doctor_rows - set(names) == set()
    assert set(names) - doctor_rows == behavioural

    # The S5.1 family is one row per `ddl.py`-owned relation, in registry order —
    # which is the order `er init` visits them in, so the doctor's tail and the
    # command's stdout name them the same way.
    assert SCHEMA_DRIFT_CHECK_NAMES == tuple(
        f"{SCHEMA_DRIFT_PREFIX}{relation}" for relation in DDL_OWNED
    )

    # Print order is S2.1's row order, then T-DOCTOR-1's six, then S5.1's: the
    # operator reads the pin table, then the behaviour, then the lake.
    assert names[-len(SCHEMA_DRIFT_CHECK_NAMES) :] == list(SCHEMA_DRIFT_CHECK_NAMES)
    tail = len(SCHEMA_DRIFT_CHECK_NAMES) + len(RUNTIME_CHECK_NAMES)
    assert names[-tail : -len(SCHEMA_DRIFT_CHECK_NAMES)] == list(RUNTIME_CHECK_NAMES)


def test_a_doctor_asserted_row_without_a_probe_is_an_error() -> None:
    """AC1's other half: the table cannot be built while a row is unasserted.

    Adding a component to S2.1 and marking it `er doctor` is a two-file change; this
    is what stops the second file being forgotten, and it fails at import rather than
    at the check that never ran.
    """
    orphan = {"nothing-probes-this": Pin("nothing-probes-this", "1.0", None, True)}
    with pytest.raises(MissingProbeError) as raised:
        build_checks(orphan)
    assert "nothing-probes-this" in str(raised.value)


def test_postgres_extension_asserted_as_postgres_scanner() -> None:
    """AC3: the extension is read under its registered name, not its install name."""
    pin = EXTENSION_PINS["postgres"]
    assert pin.install_name == "postgres"
    assert pin.registered_name == "postgres_scanner"

    # What the probe actually asks the engine. The parameter, not the docstring, is
    # what decides whether this check can ever fail.
    connection = RecordingConnection((pin.version,))
    assert extension_version(connection, pin) == pin.version
    _, parameters = connection.statements[0]
    assert parameters == [pin.registered_name]

    # And why it matters, against the pinned engine rather than against a comment:
    # there is no row named `postgres`, so the install-name filter would return
    # nothing and a check written against it would pass while asserting nothing.
    bare = duckdb.connect()
    try:
        for name, expected in ((pin.install_name, 0), (pin.registered_name, 1)):
            row = bare.execute(
                "SELECT count(*) FROM duckdb_extensions() WHERE extension_name = ?", [name]
            ).fetchone()
            assert row is not None and row[0] == expected, f"{name} matched {row} rows"
    finally:
        bare.close()


def test_failed_check_exits_1_and_still_prints_all_lines() -> None:
    """AC2: one wrong pin fails its own line, exits 1, and prints every other line.

    The wrong pin is injected into a *copy* of S2.1 — S2.1 itself is the authority the
    doctor restates, and mutating it in place would leave the table wrong for every
    test after this one.
    """
    wrong = "4.0.15"
    pins = dict(PINS)
    pins["splink"] = replace(PINS["splink"], version=wrong)
    table = pure_checks(pins)
    assert len(table) == len(PURE_COMPONENTS)

    results = run_checks(table)
    assert [result.name for result in results] == list(PURE_COMPONENTS)
    assert exit_code(results) == 1

    failed = [result for result in results if not result.ok]
    assert [result.name for result in failed] == ["splink"]
    assert failed[0].expected == wrong
    assert failed[0].actual == PINS["splink"].version
    assert failed[0].verdict == "fail"
    # Exit 1, "not 3": a pin mismatch is a check failure under S4.0's exit-code table
    # and 3 is reserved there for the five named precondition failures (S2.1).
    assert exit_code(results) != 3

    # Every other row still printed, and still passed. The doctor's contract is a
    # table, and a table that stops at the first bad row is worth less than none.
    lines = list(check_lines(results))
    assert len(lines) == len(results)
    for result, line in zip(results, lines, strict=True):
        assert result.name in line and result.expected in line and result.verdict in line


def test_the_command_prints_one_line_per_check_and_exits_1(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """AC1/AC2 through the CLI: `er doctor` is the table plus the exit code.

    ``er.doctor.CHECKS`` is substituted rather than the whole environment stubbed,
    because what is under test is the command — that it prints ``len(CHECKS)`` lines,
    that ``--json`` carries the same four columns, and that it exits 1 on a failure
    without a `runs` row or a writer lock anywhere in the path.
    """
    pins = dict(PINS)
    pins["splink"] = replace(PINS["splink"], version="4.0.15")
    table = pure_checks(pins)
    monkeypatch.setattr("er.doctor.CHECKS", table)
    # S4.0 lists lake env only for `er doctor`, so it runs without a document; a
    # developer's exported ER_CONFIG must not change what this asserts.
    monkeypatch.delenv("ER_CONFIG", raising=False)

    human = runner.invoke(app, ["doctor"])
    assert human.exit_code == 1, human.stdout
    assert len(human.stdout.strip().splitlines()) == len(table)

    machine = runner.invoke(app, ["doctor", "--json"])
    assert machine.exit_code == 1
    records = [json.loads(line) for line in machine.stdout.splitlines()]
    assert [record["check"] for record in records] == [check.name for check in table]
    assert {record["verdict"] for record in records} == {"pass", "fail"}
    assert [record for record in records if record["verdict"] == "fail"] == [
        {
            "check": "splink",
            "expected": "4.0.15",
            "actual": PINS["splink"].version,
            "verdict": "fail",
        }
    ]

    monkeypatch.setattr("er.doctor.CHECKS", pure_checks(dict(PINS)))
    healthy = runner.invoke(app, ["doctor"])
    assert healthy.exit_code == 0, healthy.stdout
