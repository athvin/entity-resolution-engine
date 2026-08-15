"""`name_norm` (DesignDoc.md S4.2; S8.4).

S4.2's macro row reads as one macro emitting three columns; it is two, and this
file tests the scalar half -- the expression applied to `given_name` and to
`family_name` alike. The array and its symmetry guarantee are
`tests/unit/dbt/test_name_variants.py`.

The punctuation rules are pinned rather than left to taste, so they are pinned
here too: `-` and `_` become a single space, `.` `'` `U+2019` `,` are removed, and
internal whitespace collapses. `O'Brien` and `OBrien` therefore agree, and so do
`Mary-Jane` and `Mary Jane` -- the two classes exist precisely because removing a
hyphen would join `mary` and `jane`, and spacing an apostrophe would split
`obrien`.

Codepoints a reader cannot tell apart on screen are built with `chr()` or derived
with `unicodedata`, never typed. The NFC and NFD spellings of the AC1 case differ
by nothing a diff viewer shows, so deriving the second from the first is what
keeps the pair from silently collapsing into one spelling asserted twice -- and
`test_nfc_and_nfd_spellings_differ` fails if it ever does.

The `hypothesis` alphabet follows `tests/unit/dbt/test_lowercase_trim.py` in
excluding U+00DF, the Turkish dotted-I pair and the U+FB01 ligature: a property
failing on those would be a property about Unicode case folding, not about
`name_norm`.
"""

from __future__ import annotations

import unicodedata

import pytest
from harness import MacroHarness
from hypothesis import given, settings
from hypothesis import strategies as st

#: U+212B ANGSTROM SIGN, which NFC replaces outright with U+00C5.
ANGSTROM_SIGN = chr(0x212B)

#: U+2019 RIGHT SINGLE QUOTATION MARK -- the apostrophe real name data carries.
CURLY_APOSTROPHE = chr(0x2019)

#: Combining acute, diaeresis and ring: the marks NFC composes and the fold drops.
COMBINING_MARKS = "".join(chr(codepoint) for codepoint in (0x0301, 0x0308, 0x030A))

#: The typed part of the alphabet; the ambiguous codepoints are appended below.
TYPED_ALPHABET = (
    " abzXYZ09"  # ASCII, including the space that collapses
    ".-_,'"  # the punctuation characters the macro has rules for
    "áäåéöüñç"  # precomposed Latin, lower
    "ÁÄÅÉÖÜ"  # ... and upper, for the casing property
    "中文"  # CJK, which neither NFC nor the fold touches
    "яЯ"  # Cyrillic ya, lower and upper
)

NAME_ALPHABET = TYPED_ALPHABET + CURLY_APOSTROPHE + COMBINING_MARKS + ANGSTROM_SIGN

#: AC1's literal case. The NFD spelling is DERIVED, so the two cannot drift into
#: being the same string typed twice.
JOSE_MARIA_NFC = unicodedata.normalize("NFC", "  José-María O'Brien ")
JOSE_MARIA_NFD = unicodedata.normalize("NFD", JOSE_MARIA_NFC)


def normalize(harness: MacroHarness, value: str | None) -> str | None:
    result: str | None = harness.eval_macro("name_norm", value)[0]["value"]
    return result


def sentinels(harness: MacroHarness) -> list[str]:
    """The S4.2 vocabulary, read from `NULL_SENTINELS` rather than restated."""
    vocabulary = harness.render_macro("NULL_SENTINELS")
    return [literal.strip().strip("'") for literal in vocabulary.split(",")]


def test_nfc_and_nfd_spellings_differ() -> None:
    """The AC1 pair is two spellings, not one string asserted twice."""
    assert JOSE_MARIA_NFC != JOSE_MARIA_NFD
    # U+212B is a codepoint NFC replaces outright, not a spelling of U+00C5.
    assert ANGSTROM_SIGN != chr(0x00C5)
    assert unicodedata.normalize("NFC", ANGSTROM_SIGN) == chr(0x00C5)


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (JOSE_MARIA_NFC, "jose maria obrien"),  # AC1, verbatim
        (JOSE_MARIA_NFD, "jose maria obrien"),  # ... and its NFD spelling
        ("MARY-JANE", "mary jane"),  # AC1: the hyphen splits
        ("Mary Jane", "mary jane"),  # ... and agrees with the spelled form
        ("O'Brien", "obrien"),  # the apostrophe is removed, never spaced
        ("OBrien", "obrien"),
        (f"O{CURLY_APOSTROPHE}Brien", "obrien"),  # U+2019, same rule
        ("Anne_Marie", "anne marie"),  # `_` is a separator, like `-`
        ("Smith, Jr.", "smith jr"),  # comma and period are removed
        ("van  der\tBerg", "van der berg"),  # internal whitespace collapses
        (f"{ANGSTROM_SIGN}sa", "asa"),  # U+212B -> U+00C5 -> `a`
    ],
)
def test_punctuation_diacritics_and_whitespace_rules(
    harness: MacroHarness, value: str, expected: str
) -> None:
    assert normalize(harness, value) == expected


# deadline=None, not a longer deadline: the first example pays the MacroHarness
# DuckDB warm-up (hypothesis measured 560ms first call, 10ms after), so ANY fixed
# deadline is a race against machine load, not a property of the macro. The
# property itself is unchanged and still runs on every generated example.
@settings(deadline=None)
@given(st.text(alphabet=NAME_ALPHABET, max_size=12))
def test_idempotent_and_casing_invariant(harness: MacroHarness, value: str) -> None:
    once = normalize(harness, value)
    # Idempotence: standardizing an already-standardized value changes nothing,
    # which is what lets `int_std_records` be rebuilt without drifting (S4.2).
    assert normalize(harness, once) == once
    # Casing invariance, since this normalizer lowercases (S8.4).
    assert normalize(harness, value.upper()) == once
    # AC2: an NFD input folds identically to its NFC spelling. `content_hash`
    # (S4.1) hashes NFC text, so the two spellings are the same evidence and this
    # macro may not turn them into two.
    assert normalize(harness, unicodedata.normalize("NFD", value)) == once
    # NULL and the empty string stay distinguishable: absence has one spelling.
    assert once != ""


def test_blank_and_sentinel_inputs_are_null(harness: MacroHarness) -> None:
    """AC1: blanks and the whole S4.2 sentinel vocabulary are absence, not evidence."""
    for blank in ("", "   ", None):
        assert normalize(harness, blank) is None

    vocabulary = sentinels(harness)
    assert vocabulary == ["", "null", "n/a", "-", "unknown"]
    for sentinel in vocabulary:
        assert normalize(harness, sentinel) is None
        # Casing and surrounding space do not smuggle a sentinel through, because
        # `null_semantics` compares the lowered, trimmed value.
        assert normalize(harness, f"  {sentinel.upper()} ") is None

    # A near miss is a name, not a sentinel: the vocabulary is exact.
    assert normalize(harness, "Nkunku") == "nkunku"
