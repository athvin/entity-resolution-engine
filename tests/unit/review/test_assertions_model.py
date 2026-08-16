"""The S4.4 assertion model, without Docker (S4.4, S4.4.1, S5.0, S8.2.1).

Two halves, and the split is the ticket's:

* the *write* half — canonicalisation and precedence — needs a relation, so `lake` is
  an in-memory database ATTACHed under the alias and holding S5's own `assertions`.
  ATTACHed rather than created as `main.assertions` because every statement in the
  module under test is written `lake.main.…` (S4.0b forbids DuckLake being the default
  catalog), and a fixture that made the table reachable unqualified would let a missing
  qualifier pass here and fail against a real lake;
* the *pure* half — the S8.2.1 parser and CONTRADICTION-1 — needs nothing at all.
  :func:`~er.review.assertions.check_contradiction_1` is a function over a sequence of
  assertions, and being able to test it with no connection is part of its contract
  (S4.4.1: the hard exit-`1` failure is `er reconcile`'s, not the checker's).

Ids come from :class:`~er.entities.ids.CountingIdFactory`, so a finding's
`assertion_id`s are the same strings in every process and an assertion about them is
about the finding rather than about the clock (S4.5.4, D10).
"""

from __future__ import annotations

from collections.abc import Iterator
from datetime import datetime
from pathlib import Path
from typing import Final

import duckdb
import pytest

from er.entities.ids import CountingIdFactory
from er.errors import ConfigError, ExitCode, exit_code_for
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER, create_table_sql
from er.review.assertions import (
    ALWAYS,
    ASSERTIONS_CSV_HEADER,
    ASSERTIONS_RELATION,
    NEVER,
    Assertion,
    AssertionConflict,
    active_assertions,
    add_assertion,
    assertion_id_for,
    check_contradiction_1,
    load_assertions_csv,
    parse_assertions_csv,
    retract_assertion,
)

ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.{ASSERTIONS_RELATION}"

#: A fixed instant for `created_at`/`retracted_at`. Nothing here compares timestamps,
#: and a clock reading would make the rows differ between two runs of the same test.
STAMP: Final = datetime(2026, 1, 1, 12, 0, 0)

#: Three record keys whose lexical order is A < B < C, so a test that passes a pair the
#: wrong way round is visibly passing it the wrong way round.
KEY_A: Final = "billing:1"
KEY_B: Final = "crm:1"
KEY_C: Final = "webforms:9"


@pytest.fixture
def lake() -> Iterator[duckdb.DuckDBPyConnection]:
    """An in-memory stand-in for the lake, holding S5's own `assertions`."""
    connection = duckdb.connect()
    connection.execute("ATTACH ':memory:' AS lake")
    connection.execute(create_table_sql(REGISTRY[ASSERTIONS_RELATION]))
    try:
        yield connection
    finally:
        connection.close()


@pytest.fixture
def ids() -> CountingIdFactory:
    """The same id sequence in every process (S4.5.4, D10)."""
    return CountingIdFactory(start=1)


def rows(connection: duckdb.DuckDBPyConnection) -> list[tuple[str, str, str, bool]]:
    """Every row as `(rec_a_key, rec_b_key, kind, active)`, in insertion order."""
    return [
        (str(a), str(b), str(kind), bool(active))
        for a, b, kind, active in connection.execute(
            f"SELECT rec_a_key, rec_b_key, kind, active FROM {ASSERTIONS} ORDER BY assertion_id"
        ).fetchall()
    ]


def assertion(assertion_id: str, a: str, b: str, kind: str, *, active: bool = True) -> Assertion:
    """One in-memory assertion, for the checker's pure half."""
    return Assertion(
        assertion_id=assertion_id,
        rec_a_key=a,
        rec_b_key=b,
        kind=kind,
        active=active,
        created_by="tester",
        created_at=STAMP,
    )


def test_canonicalises_pair_on_write(
    lake: duckdb.DuckDBPyConnection, ids: CountingIdFactory
) -> None:
    """AC1: `--a`/`--b` are unordered inputs; the row is canonical (S5.0, D9).

    The arguments are passed deliberately the wrong way round — `webforms:9` sorts
    after `crm:1` — because "canonicalised, not reflected verbatim" is only observable
    against a caller that got the order wrong.
    """
    written = add_assertion(
        lake,
        a=KEY_C,
        b=KEY_B,
        kind=ALWAYS,
        created_by="tester",
        id_factory=ids,
        created_at=STAMP,
    )

    assert written.rec_a_key < written.rec_b_key
    assert written.pair == (KEY_B, KEY_C)
    assert written.active is True
    assert written.created_by == "tester"
    assert rows(lake) == [(KEY_B, KEY_C, ALWAYS, True)]

    # Every one of S4.0's five stdout fields, and no sixth.
    assert set(written.manifest()) == {
        "assertion_id",
        "rec_a_key",
        "rec_b_key",
        "kind",
        "active",
    }

    # The symbolic resolution S8.2.1 depends on: a pair, in either order, resolves to
    # the minted id -- which is what lets a fixture refer to an assertion without ever
    # naming a ULID.
    assert assertion_id_for(lake, KEY_B, KEY_C) == written.assertion_id
    assert assertion_id_for(lake, KEY_C, KEY_B) == written.assertion_id
    assert assertion_id_for(lake, KEY_A, KEY_B) is None


def test_never_dominates_always_conflict_rejected(
    lake: duckdb.DuckDBPyConnection, ids: CountingIdFactory
) -> None:
    """AC2/AC4: a conflicting insert is refused, and retraction reopens the pair.

    Both directions are refused. S4.4's "`never` dominates `always`" is about the
    ORDER edge adjustment applies the two, not about which insert wins: at write time
    there is at most one active row per pair, so the second kind is rejected whichever
    way round the two arrive.
    """
    first = add_assertion(
        lake, a=KEY_B, b=KEY_C, kind=ALWAYS, created_by="tester", id_factory=ids, created_at=STAMP
    )

    with pytest.raises(AssertionConflict) as refused:
        add_assertion(
            lake,
            a=KEY_C,
            b=KEY_B,
            kind=NEVER,
            created_by="tester",
            id_factory=ids,
            created_at=STAMP,
        )

    # The message names the row an operator has to retract to proceed, and the
    # refusal is S4.0 exit 1 -- a stage failure, not a config error.
    assert first.assertion_id in str(refused.value)
    assert refused.value.existing.assertion_id == first.assertion_id
    assert exit_code_for(refused.value) == int(ExitCode.STAGE_FAILURE)
    assert rows(lake) == [(KEY_B, KEY_C, ALWAYS, True)]

    # Re-adding the SAME kind is not a conflict and writes no second row: S5.0 permits
    # one active row per pair, and S4.0 gives `er assert add` no exit for "already so".
    again = add_assertion(
        lake, a=KEY_B, b=KEY_C, kind=ALWAYS, created_by="tester", id_factory=ids, created_at=STAMP
    )
    assert again.assertion_id == first.assertion_id
    assert rows(lake) == [(KEY_B, KEY_C, ALWAYS, True)]

    retracted = retract_assertion(lake, first.assertion_id, retracted_by="tester")

    assert retracted.active is False
    assert retracted.retracted_by == "tester"
    assert retracted.retracted_at is not None
    # Retraction is a stamp, never a DELETE: the row count is unchanged, which is what
    # makes the assertion delta between two runs computable (S4.4, S4.5.1).
    assert rows(lake) == [(KEY_B, KEY_C, ALWAYS, False)]
    assert active_assertions(lake) == []

    opposite = add_assertion(
        lake, a=KEY_B, b=KEY_C, kind=NEVER, created_by="tester", id_factory=ids, created_at=STAMP
    )
    assert opposite.kind == NEVER
    assert rows(lake) == [(KEY_B, KEY_C, ALWAYS, False), (KEY_B, KEY_C, NEVER, True)]
    assert [row.assertion_id for row in active_assertions(lake)] == [opposite.assertion_id]


def write_csv(path: Path, *lines: str) -> Path:
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def test_parse_assertions_csv_header_and_phase_vocabulary(tmp_path: Path) -> None:
    """AC6: the S8.2.1 header is literal and the phase vocabulary is closed."""
    good = write_csv(
        tmp_path / "assertions.csv",
        ASSERTIONS_CSV_HEADER,
        f"base,{KEY_B},{KEY_C},always,tester,keep together",
        f"batch,{KEY_C},{KEY_A},never,tester,\\N",
    )

    parsed = parse_assertions_csv(good)

    assert [row.phase for row in parsed] == ["base", "batch"]
    assert [row.kind for row in parsed] == [ALWAYS, NEVER]
    # The keys are carried verbatim; canonicalisation is a write-time step through the
    # single helper of S5.0, and doing it at parse time would be a second ordering site.
    assert parsed[1].rec_a_key == KEY_C
    assert parsed[0].note == "keep together"
    # `\N` is NULL; an empty field would be the empty string, a distinct value (S8.2.1).
    assert parsed[1].note is None

    # A header that is not byte-equal is refused even when it is a permutation of the
    # right names: the file is read positionally, so a reordered header silently lands
    # a `phase` value in `rec_a_key`.
    reordered = write_csv(
        tmp_path / "reordered.csv",
        "phase,rec_b_key,rec_a_key,kind,created_by,note",
        f"base,{KEY_C},{KEY_B},always,tester,\\N",
    )
    with pytest.raises(ConfigError) as bad_header:
        parse_assertions_csv(reordered)
    assert ASSERTIONS_CSV_HEADER in str(bad_header.value)
    assert exit_code_for(bad_header.value) == int(ExitCode.CONFIG)

    # An extra column is the same failure for the same reason.
    with pytest.raises(ConfigError):
        parse_assertions_csv(
            write_csv(
                tmp_path / "extra.csv",
                f"{ASSERTIONS_CSV_HEADER},assertion_id",
                f"base,{KEY_B},{KEY_C},always,tester,\\N,01JQZ8XKQ4T7VN3M2B9CDEFGHJ",
            )
        )

    unknown_phase = write_csv(
        tmp_path / "phase.csv",
        ASSERTIONS_CSV_HEADER,
        f"initial,{KEY_B},{KEY_C},always,tester,\\N",
    )
    with pytest.raises(ConfigError) as bad_phase:
        parse_assertions_csv(unknown_phase)
    assert "initial" in str(bad_phase.value)
    assert exit_code_for(bad_phase.value) == int(ExitCode.CONFIG)


def test_load_applies_every_row_and_is_idempotent(
    lake: duckdb.DuckDBPyConnection, ids: CountingIdFactory, tmp_path: Path
) -> None:
    """AC5: `load` applies every row of the file; a second load inserts nothing.

    Every row, not the rows of one phase: `assertions.csv` carries `phase` so that a
    scenario driver can slice the file before handing it over (S8.2.1).
    """
    path = write_csv(
        tmp_path / "assertions.csv",
        ASSERTIONS_CSV_HEADER,
        f"base,{KEY_C},{KEY_B},always,tester,\\N",
        f"batch,{KEY_C},{KEY_A},never,tester,\\N",
    )

    applied = load_assertions_csv(lake, path, id_factory=ids, created_at=STAMP)

    assert len(applied) == 2
    assert rows(lake) == [(KEY_B, KEY_C, ALWAYS, True), (KEY_A, KEY_C, NEVER, True)]

    again = load_assertions_csv(lake, path, id_factory=ids, created_at=STAMP)

    assert again == []
    assert rows(lake) == [(KEY_B, KEY_C, ALWAYS, True), (KEY_A, KEY_C, NEVER, True)]


def test_check_contradiction_1_finds_always_closure_violation() -> None:
    """AC7: `always(a,b) ∧ always(b,c) ∧ never(a,c)` is unsatisfiable (S4.4.1).

    One finding, carrying all three `assertion_id`s and the closure component — which
    is exactly what S4.4.1 requires in `run_stages.error_detail`.
    """
    always_ab = assertion("A1", KEY_A, KEY_B, ALWAYS)
    always_bc = assertion("A2", KEY_B, KEY_C, ALWAYS)
    never_ac = assertion("A3", KEY_A, KEY_C, NEVER)

    findings = check_contradiction_1([always_ab, always_bc, never_ac])

    assert len(findings) == 1
    finding = findings[0]
    assert finding.never_assertion_id == never_ac.assertion_id
    assert set(finding.assertion_ids) == {"A1", "A2", "A3"}
    assert finding.component == frozenset({KEY_A, KEY_B, KEY_C})
    assert "CONTRADICTION-1" in finding.detail()
    for offender in finding.assertion_ids:
        assert offender in finding.detail()


def test_check_contradiction_1_clean_set_returns_empty() -> None:
    """AC7: retracting the `never`, or moving it out of the component, clears it."""
    always_ab = assertion("A1", KEY_A, KEY_B, ALWAYS)
    always_bc = assertion("A2", KEY_B, KEY_C, ALWAYS)
    never_ac = assertion("A3", KEY_A, KEY_C, NEVER)

    # A retracted `never` is not a constraint. That difference is the whole reason
    # retraction is a stamp rather than a delete (S4.4).
    retracted = assertion("A3", KEY_A, KEY_C, NEVER, active=False)
    assert check_contradiction_1([always_ab, always_bc, retracted]) == []

    # Endpoints in two different always-components are satisfiable: the partition can
    # simply keep them apart, so this is not CONTRADICTION-1 and must not fail a run.
    other = assertion("A4", "crm:2", "crm:3", ALWAYS)
    assert check_contradiction_1([always_ab, other, assertion("A5", KEY_A, "crm:2", NEVER)]) == []

    # A `never` with no always edge at either endpoint is likewise clean.
    assert check_contradiction_1([never_ac]) == []

    # And the same set WITH the never active is not, so the three cases above are
    # clean for their own reasons rather than because the checker never fires.
    assert len(check_contradiction_1([always_ab, always_bc, never_ac])) == 1
