"""Regenerate the er-std address-parser golden oracle from the Python RegexV1Parser.

    uv run python rust/crates/er-std/tests/gen_golden.py

The Rust port (src/address.rs) must reproduce addr_number/addr_street/addr_unit
for every input byte-for-byte. Inputs exercise each branch of the S4.2 v1 grammar
(units, PO boxes, 221b vs 5th, sentinels, punctuation, ASCII-only whitespace incl
U+00A0, NFC+lowercase of accented text).
"""

import json
from pathlib import Path

from er.std.address import RegexV1Parser

HERE = Path(__file__).resolve().parent
PARSER = RegexV1Parser()

INPUTS = [
    None, "", "   ", "null", "N/A", "n/a", "-", "Unknown", "UNKNOWN",
    "123 Main St", "123 Main Street, Apt 5", "123 Main St Apt 4",
    "123 Main St #4B", "123 Main St #12-B", "123 Main St Unit A-1",
    "123 Main St Suite 200", "123 Main St Ste 200", "123 Main St Floor 2",
    "123 Main St Fl 2", "123 Main St Rm 3", "123 Main St Room 3",
    "123 Main St Apartment 9", "221B Baker St", "5th Avenue", "10 Downing St",
    "PO Box 123", "P.O. Box 5", "PO Box 9", "Post Office Box 9", "po box",
    "Apt 5", "123", "123  Main   St", "123\tMain\nSt", "123 Main St.",
    "10 Café Street", "123 Main St", "  123 Main St  ",
    "#5", "123 Main St #5", "Suite 200", "1600 Pennsylvania Ave NW",
]


def main() -> None:
    cases = []
    for line in INPUTS:
        c = PARSER.parse(line)
        cases.append({
            "input": line,
            "addr_number": c.addr_number,
            "addr_street": c.addr_street,
            "addr_unit": c.addr_unit,
        })
    doc = {
        "_comment": "Golden RegexV1Parser oracle. Regenerate: uv run python rust/crates/er-std/tests/gen_golden.py",
        "version": PARSER.version,
        "cases": cases,
    }
    (HERE / "address_golden.json").write_text(
        json.dumps(doc, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    print(f"wrote {len(cases)} address cases")


if __name__ == "__main__":
    main()
