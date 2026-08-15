"""`null_semantics` (DesignDoc.md S4.2; S8.4).

Two things are being pinned, and both are division-of-labour claims rather than
string-munging ones.

S4.2 lists the sentinel vocabulary in mixed case (`'NULL'`, `'N/A'`), so the match
is case-insensitive on the trimmed value. If it were not, `'null'` and `'n/a'`
would reach the matcher as literal evidence and two records agreeing on the string
`'n/a'` would look like two records agreeing on a value.

And S4.2 assigns placeholder emails to `email_norm` and to no other macro, which
is why `'test@test.com'` -- the address `base_10` carries for exactly this purpose
-- must pass through here untouched. A `null_semantics` that also nulled
placeholders would pass every test written about placeholders while quietly making
the ownership rule unfalsifiable.
"""

from __future__ import annotations

import pytest
from harness import MacroHarness
from hypothesis import given
from hypothesis import strategies as st

# S4.2's vocabulary in both cases, plus the blank that trims to the empty string.
SENTINELS = ["", "NULL", "null", "N/A", "n/a", "-", "unknown", "UNKNOWN", "   "]

# Near misses. Each is one edit away from a sentinel and none of them is one.
NON_SENTINELS = ["none", "na", "--", "0", "test@test.com", "N/A/B", "unknown value"]

SENTINEL_ALPHABET = " abnulkow/-0@.NULKOW"


def apply(harness: MacroHarness, value: str | None) -> str | None:
    result: str | None = harness.eval_macro("null_semantics", value)[0]["value"]
    return result


@pytest.mark.parametrize("value", SENTINELS)
def test_sentinel_vocabulary_maps_to_null(harness: MacroHarness, value: str) -> None:
    assert apply(harness, value) is None


@pytest.mark.parametrize("value", NON_SENTINELS)
def test_non_sentinels_including_placeholder_email_pass_through(
    harness: MacroHarness, value: str
) -> None:
    # Verbatim: this macro decides presence, `lowercase_trim` decides spelling.
    assert apply(harness, value) == value


def test_vocabulary_is_declared_once(harness: MacroHarness) -> None:
    """`NULL_SENTINELS` is the only place the vocabulary is written (S4.2)."""
    vocabulary = harness.render_macro("NULL_SENTINELS")
    assert vocabulary == "'', 'null', 'n/a', '-', 'unknown'"
    assert vocabulary in harness.render_macro("null_semantics", "col")


@given(st.text(alphabet=SENTINEL_ALPHABET, max_size=10))
def test_idempotent_and_case_insensitive(harness: MacroHarness, value: str) -> None:
    once = apply(harness, value)
    assert apply(harness, once) == once
    # Not casing INVARIANCE: this macro does not lowercase, and a non-sentinel is
    # returned verbatim. What must be casing-invariant is the sentinel DECISION,
    # which is the thing S4.2's mixed-case listing puts at risk.
    assert (apply(harness, value.upper()) is None) == (once is None)
    # The empty string is a sentinel, so it can never be an output.
    assert once != ""
