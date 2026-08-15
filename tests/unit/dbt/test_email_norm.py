"""`email_norm` (DesignDoc.md S4.2, S5, S6; S8.4).

`email_norm` is the one standardization macro that is not a scalar: S5 gives
`int_std_records` both an `email` and an `email_valid BOOLEAN`, and S4.6's
`validated` survivorship rule orders `<attr>_valid DESC NULLS LAST`. That ordering
is why `email_valid` is three-valued and why the three values are not
interchangeable -- a configured placeholder must come back NULL (no evidence) and
never `false`, which would rank a record carrying `test@test.com` BELOW a record
carrying no email at all.

Every setting is read from the `standardization` var, so the tests override the
var rather than editing anything: a run with a different `email_placeholders` list
produces a different answer from the same macro source, which is what "the config
is the single source of truth" means operationally (S4.2, S6).
"""

from __future__ import annotations

from typing import Any

import pytest
from harness import MacroHarness, split_projections
from hypothesis import given
from hypothesis import strategies as st

# Restricted to codepoints whose case mapping round-trips, for the same reason
# tests/unit/dbt/test_lowercase_trim.py restricts its alphabet.
EMAIL_ALPHABET = " abzXY09.+-_@áÅ"


def standardization(dbt_vars: dict[str, Any], **overrides: Any) -> dict[str, Any]:
    """The S6 `standardization` block with named keys replaced."""
    return {"standardization": {**dbt_vars["standardization"], **overrides}}


def norm(
    harness: MacroHarness, value: str | None, vars: dict[str, Any] | None = None
) -> dict[str, Any]:
    row: dict[str, Any] = harness.eval_macro("email_norm", value, vars=vars)[0]
    return row


def test_plus_addressing_honoured_in_both_settings(
    harness: MacroHarness, dbt_vars: dict[str, Any]
) -> None:
    """AC4: `configs/test.yaml` keeps the plus tag; flipping the flag strips it."""
    assert dbt_vars["standardization"]["email_strip_plus_addressing"] is False

    kept = norm(harness, "Bob+news@Example.COM ")
    assert kept == {"email": "bob+news@example.com", "email_valid": True}

    stripped = norm(
        harness,
        "Bob+news@Example.COM ",
        standardization(dbt_vars, email_strip_plus_addressing=True),
    )
    assert stripped == {"email": "bob@example.com", "email_valid": True}

    # Only the local part, and only up to the first `@`: a `+` in the domain is
    # part of the address, not a tag.
    assert norm(
        harness,
        "bob@example.com",
        standardization(dbt_vars, email_strip_plus_addressing=True),
    ) == {"email": "bob@example.com", "email_valid": True}


def test_placeholders_null_email_and_email_valid(
    harness: MacroHarness, dbt_vars: dict[str, Any]
) -> None:
    """AC5: a placeholder is absence, an unparsable value is a rejected observation."""
    placeholders = dbt_vars["standardization"]["email_placeholders"]
    assert "test@test.com" in placeholders

    for placeholder in placeholders:
        assert norm(harness, placeholder) == {"email": None, "email_valid": None}
        # Casing and surrounding space do not smuggle a placeholder through.
        assert norm(harness, f"  {placeholder.upper()} ") == {
            "email": None,
            "email_valid": None,
        }

    # Non-empty and unparsable: no email, but `false` records that we looked.
    assert norm(harness, "not-an-email") == {"email": None, "email_valid": False}
    # Absent: no email and no claim either way.
    assert norm(harness, "") == {"email": None, "email_valid": None}
    assert norm(harness, None) == {"email": None, "email_valid": None}


def test_emits_email_and_email_valid_aliases(
    harness: MacroHarness, dbt_vars: dict[str, Any]
) -> None:
    """AC6: exactly two projections, the S5 names, and the var drives the answer."""
    projections = split_projections(harness.render_macro("email_norm", "email_address"))
    assert len(projections) == 2
    assert projections[0].endswith("as email")
    assert projections[1].endswith("as email_valid")

    # Same macro source, a different placeholder list, a different answer.
    assert norm(harness, "ada@example.com") == {"email": "ada@example.com", "email_valid": True}
    assert norm(
        harness,
        "ada@example.com",
        standardization(dbt_vars, email_placeholders=["ada@example.com"]),
    ) == {"email": None, "email_valid": None}
    # An empty placeholder list is legal under S6.1 V16 and must render valid SQL.
    assert norm(
        harness,
        "test@test.com",
        standardization(dbt_vars, email_placeholders=[]),
    ) == {"email": "test@test.com", "email_valid": True}


@pytest.mark.parametrize("strip_plus", [False, True])
@given(st.text(alphabet=EMAIL_ALPHABET, max_size=16))
def test_idempotent_and_casing_invariant(
    harness: MacroHarness, dbt_vars: dict[str, Any], strip_plus: bool, value: str
) -> None:
    overrides = standardization(dbt_vars, email_strip_plus_addressing=strip_plus)
    once = norm(harness, value, overrides)["email"]
    assert norm(harness, once, overrides)["email"] == once
    assert norm(harness, value.upper(), overrides)["email"] == once
    assert once != ""
