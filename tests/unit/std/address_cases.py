"""The committed address case table (DesignDoc.md S4.2, S5, S8.2).

ONE table, read by both sides of the grammar: `tests/unit/std/test_address_parser.py`
runs it through `RegexV1Parser` and `tests/unit/dbt/test_address.py` runs it through
`dbt/macros/std/address_parse.sql`, then compares the two field for field. Two
tables would let the SQL and the Python drift apart while both stayed green against
their own inputs, which is exactly the failure T-BLK-1 exists to catch elsewhere.

The expectations are committed here rather than computed from either implementation:
a table derived from the code under test asserts that the code agrees with itself.

`why` is not decoration -- it names the rule of the v1 grammar each row pins, so a
row that stops failing when the grammar changes is traceable to the sentence it was
protecting.
"""

from __future__ import annotations

import unicodedata
from dataclasses import dataclass

__all__ = ["ADDRESS_CASES", "AddressCase"]


@dataclass(frozen=True, slots=True)
class AddressCase:
    """One address line and the three components S4.2 says it componentizes into."""

    line: str | None
    addr_number: str | None
    addr_street: str | None
    addr_unit: str | None
    why: str

    @property
    def components(self) -> tuple[str | None, str | None, str | None]:
        """The expectation as the field-for-field tuple both sides are compared as."""
        return (self.addr_number, self.addr_street, self.addr_unit)


#: A precomposed non-ASCII street, and the SAME name spelled decomposed. The second
#: is DERIVED so the pair cannot silently collapse into one spelling asserted twice
#: -- `test_nfc_and_nfd_spellings_differ` fails if it ever does. Both must land on
#: the NFC spelling, because `content_hash` (S4.1) hashes NFC text.
CANON_NFC = unicodedata.normalize("NFC", "12 Cañon Rd")
CANON_NFD = unicodedata.normalize("NFD", CANON_NFC)

#: Every case both implementations are held to. AC2's ten required inputs are all
#: present; the rest cover the keyword vocabulary, the number grammar and the
#: normalization steps.
ADDRESS_CASES: tuple[AddressCase, ...] = (
    AddressCase("123 Main St", "123", "main st", None, "number + street, no unit"),
    AddressCase("123 Main St Apt 4B", "123", "main st", "apt 4b", "the AC3 case, verbatim"),
    AddressCase(
        "123 N Main Street Unit 12",
        "123",
        "n main street",
        "unit 12",
        "a directional prefix stays in the street; `unit` is a keyword",
    ),
    AddressCase("123  Main   St", "123", "main st", None, "internal whitespace collapses"),
    AddressCase("  742 Evergreen Terrace  ", "742", "evergreen terrace", None, "outer trim"),
    AddressCase(
        "123 Main St, Apt 4B", "123", "main st", "apt 4b", "the comma is stripped, not a token"
    ),
    AddressCase("456 Oak Ave Ste 200", "456", "oak ave", "ste 200", "`ste` keyword"),
    AddressCase("99 Maple Dr Suite 10", "99", "maple dr", "suite 10", "`suite` keyword"),
    AddressCase("88 Cedar Ln Rm 4", "88", "cedar ln", "rm 4", "`rm` keyword"),
    AddressCase(
        "321 Birch Blvd Apartment 9C", "321", "birch blvd", "apartment 9c", "`apartment` keyword"
    ),
    AddressCase("12a Baker St Floor 2", "12a", "baker st", "floor 2", "`floor` keyword"),
    AddressCase("789 Elm Rd #3", "789", "elm rd", "#3", "the `#` form takes no space"),
    AddressCase("221B Baker St", "221b", "baker st", None, "digits plus one letter is a number"),
    AddressCase(
        "1600 Pennsylvania Ave NW",
        "1600",
        "pennsylvania ave nw",
        None,
        "a trailing non-keyword token is street, not unit",
    ),
    AddressCase(
        "55 Pine St Apt 4B Unit 12",
        "55",
        "pine st apt 4b",
        "unit 12",
        "only the TRAILING unit phrase is taken, which is what makes the parse idempotent",
    ),
    AddressCase("Main St", None, "main st", None, "street-only: a street with no number"),
    AddressCase(
        "5th Avenue",
        None,
        "5th avenue",
        None,
        "`5th` is not a number: the boundary after the optional letter fails",
    ),
    AddressCase(CANON_NFC, "12", "cañon rd", None, "NFC survives the fold"),
    AddressCase(CANON_NFD, "12", "cañon rd", None, "... and NFD lands on the same spelling"),
    AddressCase("PO Box 7", None, None, None, "a PO box is rejected, never componentized"),
    AddressCase("P.O. Box 7", None, None, None, "... in the spelling the period strip leaves"),
    AddressCase("Post Office Box 12", None, None, None, "... and spelled out"),
    AddressCase("Apt 4B", None, None, None, "unit-only: no street, so no components"),
    AddressCase("#3", None, None, None, "the `#` form alone is a unit-only line too"),
    AddressCase("123", None, None, None, "a bare number is not an address"),
    AddressCase("", None, None, None, "the empty string is absence"),
    AddressCase("   ", None, None, None, "and so is whitespace"),
    AddressCase("unknown", None, None, None, "a `null_semantics` sentinel"),
    AddressCase("N/A", None, None, None, "... in the S4.2 mixed-case spelling"),
    AddressCase("-", None, None, None, "... and the one-character one"),
    AddressCase(None, None, None, None, "NULL in, NULL out"),
)
