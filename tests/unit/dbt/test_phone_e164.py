"""`phone_e164` (DesignDoc.md S4.2, S5, S6, S6.1; S8.2, S8.4).

`phone_e164` is the second of the two non-scalar standardization macros: S5 gives
`int_std_records` both a `phone_e164` and a `phone_valid BOOLEAN`, and S6.1 V4
makes that second column the precondition for `validated` appearing in the phone
survivorship chain. So `phone_valid` is three-valued for the same reason
`email_valid` is (S4.6 orders `<attr>_valid DESC NULLS LAST`): a record with no
phone at all must not be outranked by one whose phone we looked at and rejected.

S8.2's drifted-phone trap is the reason the macro exists in the shape it does --
`(415) 555-0132`, `415-555-0132` and `+14155550132` are one persona's one number,
and `phone_e164` is the S6 `phone_exact` blocking expression, so anything short of
one identical rendering leaves those three records unblocked and unmatched. The
property test generalises the trap: every NANP number, under every drift format,
collapses to the same E.164 string and stays there when fed back in.

The region is read from the `standardization` var, never hard-coded, so the tests
override the var rather than editing anything -- a `GB` run and a `US` run produce
different answers from one macro source, which is what S6's "the config is the
single source of truth" means operationally.
"""

from __future__ import annotations

from typing import Any

from harness import MacroHarness, split_projections
from hypothesis import example, given, settings
from hypothesis import strategies as st

#: The `base_10` drifted-phone persona (S8.2) and the one value its three records
#: must share once standardized.
TRAP_E164 = "+14155550132"
BASE_10_DRIFT = ("(415) 555-0132", "415-555-0132", "+14155550132")

#: Every spelling of one NANP number this macro must collapse. The first three are
#: `base_10`'s; the rest are the S4.2 formats a real feed also delivers.
DRIFT_FORMATS = (
    "({npa}) {nxx}-{line}",
    "{npa}-{nxx}-{line}",
    "+1{npa}{nxx}{line}",
    "1-{npa}-{nxx}-{line}",
    "{npa}.{nxx}.{line}",
    "  {npa}{nxx}{line} ",
)

#: NANP structure: an area code and an exchange code never begin with 0 or 1.
NANP_AREA = st.from_regex(r"\A[2-9][0-9]{2}\Z")
NANP_EXCHANGE = st.from_regex(r"\A[2-9][0-9]{2}\Z")
NANP_LINE = st.from_regex(r"\A[0-9]{4}\Z")


def standardization(dbt_vars: dict[str, Any], **overrides: Any) -> dict[str, Any]:
    """The S6 `standardization` block with named keys replaced."""
    return {"standardization": {**dbt_vars["standardization"], **overrides}}


def norm(
    harness: MacroHarness, value: str | None, vars: dict[str, Any] | None = None
) -> dict[str, Any]:
    row: dict[str, Any] = harness.eval_macro("phone_e164", value, vars=vars)[0]
    return row


def sql_vocabulary(harness: MacroHarness) -> tuple[str, ...]:
    """The S4.2 sentinel vocabulary, read from `NULL_SENTINELS` rather than restated."""
    rendered = harness.render_macro("NULL_SENTINELS")
    return tuple(literal.strip().strip("'") for literal in rendered.split(","))


def test_drift_formats_collapse_to_one_e164_value(harness: MacroHarness) -> None:
    """AC1: `base_10`'s three spellings of one number produce one blocking key."""
    rendered = {norm(harness, drift)["phone_e164"] for drift in BASE_10_DRIFT}
    assert rendered == {TRAP_E164}

    for drift in BASE_10_DRIFT:
        assert norm(harness, drift) == {"phone_e164": TRAP_E164, "phone_valid": True}


@settings(
    # The property is about the rendering, not about how fast an in-process DuckDB
    # answers: each example runs two dozen renders and queries, and a wall-clock
    # deadline over that measures the harness rather than the macro.
    deadline=None,
    max_examples=25,
)
@given(npa=NANP_AREA, nxx=NANP_EXCHANGE, line=NANP_LINE)
@example(npa="415", nxx="555", line="0132")
def test_e164_round_trip_and_idempotence_property(
    harness: MacroHarness, npa: str, nxx: str, line: str
) -> None:
    """AC2: every drift format of every NANP number renders once and stays there."""
    expected = f"+1{npa}{nxx}{line}"
    for drift_format in DRIFT_FORMATS:
        drifted = drift_format.format(npa=npa, nxx=nxx, line=line)
        once = norm(harness, drifted)
        assert once == {"phone_e164": expected, "phone_valid": True}
        # `phone_e164(phone_e164(x)) == phone_e164(x)`: the rendered value is itself
        # a legal input, and an E.164 input is never re-prefixed.
        assert norm(harness, once["phone_e164"]) == once


def test_null_blank_and_sentinel_inputs_yield_null_validity(harness: MacroHarness) -> None:
    """AC3: absence is NULL on both columns -- never `false`, which is a rejection."""
    absent = {"phone_e164": None, "phone_valid": None}
    assert norm(harness, None) == absent
    assert norm(harness, "") == absent
    assert norm(harness, "   ") == absent

    for sentinel in sql_vocabulary(harness):
        assert norm(harness, sentinel) == absent
        # Casing and surrounding space do not smuggle a sentinel through.
        assert norm(harness, f"  {sentinel.upper()} ") == absent


def test_unrenderable_input_is_false_not_null(harness: MacroHarness) -> None:
    """AC3: a non-empty value we looked at and rejected records `false`."""
    rejected = {"phone_e164": None, "phone_valid": False}
    # A 7-digit local number carries no area code, so no E.164 number follows from it.
    assert norm(harness, "555-0132") == rejected
    assert norm(harness, "abc") == rejected
    # A `+` with nothing behind it, and a digit run longer than E.164's 15.
    assert norm(harness, "+") == rejected
    assert norm(harness, "+1234567890123456") == rejected


def test_existing_e164_is_preserved(harness: MacroHarness, dbt_vars: dict[str, Any]) -> None:
    """AC4: a `+` value keeps its digits verbatim and is never given a second prefix."""
    assert norm(harness, "+442071838750") == {
        "phone_e164": "+442071838750",
        "phone_valid": True,
    }
    # Formatting is still stripped -- the blocking key is digits and a `+`.
    assert norm(harness, " +44 20 7183 8750 ") == {
        "phone_e164": "+442071838750",
        "phone_valid": True,
    }
    # The `+` branch is region-independent: an already-E.164 number is not the
    # default region's business, so a `GB` run renders it identically.
    assert norm(
        harness,
        "+442071838750",
        standardization(dbt_vars, phone_default_region="GB"),
    ) == {"phone_e164": "+442071838750", "phone_valid": True}


def test_non_us_region_degrades_to_null(harness: MacroHarness, dbt_vars: dict[str, Any]) -> None:
    """AC5, AC6: a region v1 cannot render yields NULL, never a guessed `+1`."""
    assert dbt_vars["standardization"]["phone_default_region"] == "US"
    gb = standardization(dbt_vars, phone_default_region="GB")

    for bare in ("4155550132", "(415) 555-0132", "1-415-555-0132"):
        assert norm(harness, bare, gb) == {"phone_e164": None, "phone_valid": False}
        # Same macro source, the shipped region, a different answer.
        assert norm(harness, bare)["phone_e164"] == TRAP_E164

    # The NANP rendering is keyed on the region, so it is absent from a `GB` render.
    gb_sql = harness.render_macro("phone_e164", "phone", vars=gb)
    assert "'+1'" not in gb_sql
    assert "'+1'" in harness.render_macro("phone_e164", "phone")


def test_emits_phone_e164_and_phone_valid_aliases(harness: MacroHarness) -> None:
    """AC6, AC7: exactly two projections, the S5 names, and no region literal."""
    rendered = harness.render_macro("phone_e164", "phone")
    projections = split_projections(rendered)
    assert len(projections) == 2
    assert projections[0].endswith("as phone_e164")
    assert projections[1].endswith("as phone_valid")

    # The region is resolved when the macro is rendered, so the SQL that reaches
    # DuckDB names no region at all -- there is nothing here to edit when the var
    # changes, which is what makes S6 the only source of the setting.
    assert "'US'" not in rendered
    assert "'us'" not in rendered
