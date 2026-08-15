"""`lowercase_trim` (DesignDoc.md S4.2; S8.4).

S8.4 asks for property-based coverage whose strategies "draw from the Unicode
ranges the NFC step must handle", so the alphabet below is chosen rather than
generic: precomposed Latin, the combining marks that compose into it, a codepoint
whose NFC form is a DIFFERENT codepoint (ANGSTROM SIGN), CJK, Cyrillic, and the
space that `trim` removes.

Deliberately excluded, each for a reason that would otherwise make the
casing-invariance property false for reasons that have nothing to do with this
macro: U+00DF (uppercases to two codepoints), the Turkish dotted-I pair U+0131 and
U+0130 (whose case mappings are not round-trips), and the U+FB01 ligature (folded
by NFKC, which is not the normalization S4.1 hashes under). A property failing on
those would be a property about Unicode case folding, not about `lowercase_trim`.

Every non-ASCII literal below is written as an escape. These cases turn on which
codepoints a string is made of -- U+00C5 against U+0041 U+030A against U+212B --
and three strings that render identically in an editor would make the table
unreadable exactly where it carries the most information.
"""

from __future__ import annotations

import pytest
from harness import MacroHarness
from hypothesis import given
from hypothesis import strategies as st

NFC_ALPHABET = (
    " abzXYZ09.-_"  # ASCII, including the space `trim` removes
    "áäåéöüñç"  # precomposed Latin, lower
    "ÁÄÅÉÖÜ"  # ... and upper, for the casing property
    "́̈̊"  # combining acute, diaeresis, ring: NFC's composition inputs
    "Å"  # ANGSTROM SIGN -> U+00C5 under NFC: a codepoint NFC replaces outright
    "中文"  # CJK, which NFC and case folding both leave alone
    "яЯ"  # Cyrillic ya, lower and upper
)

#: '  ASA ' with a ring over the A, in the three spellings that must agree.
ASA_NFC = "  ÅSA "
ASA_NFD = "  ÅSA "
ASA_ANGSTROM = "  ÅSA "


def normalize(harness: MacroHarness, value: str | None) -> str | None:
    result: str | None = harness.eval_macro("lowercase_trim", value)[0]["value"]
    return result


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (ASA_NFC, "åsa"),
        (ASA_NFD, "åsa"),
        (ASA_ANGSTROM, "åsa"),
        ("", None),  # empty string -> NULL, never ''
        ("   ", None),  # blank -> NULL by the same rule, after trim
        (None, None),  # NULL in, NULL out
    ],
)
def test_nfc_trim_lower_and_empty_to_null(
    harness: MacroHarness, value: str | None, expected: str | None
) -> None:
    assert normalize(harness, value) == expected


@given(st.text(alphabet=NFC_ALPHABET, max_size=12))
def test_idempotent_and_casing_invariant(harness: MacroHarness, value: str) -> None:
    once = normalize(harness, value)
    # Idempotence: standardizing an already-standardized value changes nothing,
    # which is what lets `int_std_records` be rebuilt without drifting (S4.2).
    assert normalize(harness, once) == once
    # Casing invariance, since this normalizer lowercases (S8.4).
    assert normalize(harness, value.upper()) == once
    # NULL and the empty string stay distinguishable: absence has one spelling.
    assert once != ""
