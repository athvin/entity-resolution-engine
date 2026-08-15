"""`address_parse` (DesignDoc.md S4.2, S5, S6; S8.1).

The macro and `src/er/std/address.py::RegexV1Parser` are TWO implementations of one
grammar. That is the drift shape T-BLK-1 exists to catch between the dbt blocking
keys and Splink's, and it is caught here the same way: both sides run over one
committed case table and are compared field for field. Neither side's own
assertions could catch a grammar that moved in both files but not identically, and
neither side's could catch one that moved in only one.

The comparison is made twice over, at two levels, because they fail differently:

* the three regex patterns are asserted to be the byte-identical strings both files
  compile, which localises a drift to the pattern that moved, and
* every case is parsed by both, which catches a drift the patterns do not explain
  -- a different normalization order, a different trim, a different NULL rule.

S5 owns the six alias names and S6 owns which of them are parsed: `addr_city`,
`addr_region` and `addr_postal` map directly from their own source columns, so an
`address_line` change may never move them.

No dbt subprocess, no lake, no new dependency: one in-process DuckDB connection,
which is what S8.1's Unit row describes (AC7).
"""

from __future__ import annotations

import re

from harness import MacroHarness, split_projections
from unit.std.address_cases import ADDRESS_CASES

from er.std import address

#: The six projections S5 names on `stg_<source>` and `int_std_records`, in the
#: order the macro emits them.
ADDR_COLUMNS = (
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
)

#: The three the v1 grammar parses. The other three are pass-throughs (S6).
PARSED_COLUMNS = ("addr_number", "addr_street", "addr_unit")

ALIAS_RE = re.compile(r"\bas\s+([a-z_]+)$")


def sql_vocabulary(harness: MacroHarness) -> tuple[str, ...]:
    """The S4.2 sentinel vocabulary, read from `NULL_SENTINELS` rather than restated."""
    rendered = harness.render_macro("NULL_SENTINELS")
    return tuple(literal.strip().strip("'") for literal in rendered.split(","))


def test_macro_matches_the_python_oracle_on_every_case(harness: MacroHarness) -> None:
    """AC1: the SQL macro and the Python oracle never disagree."""
    # The grammar itself, declared once per side. A pattern that changed in one
    # file and not the other fails here, naming the pattern.
    assert harness.render_macro("ADDRESS_UNIT_PATTERN") == address.ADDRESS_UNIT_PATTERN
    assert harness.render_macro("ADDRESS_PO_BOX_PATTERN") == address.ADDRESS_PO_BOX_PATTERN
    assert harness.render_macro("ADDRESS_NUMBER_PATTERN") == address.ADDRESS_NUMBER_PATTERN
    # ... and the sentinel vocabulary the normalization ends with, which S4.2
    # declares exactly once, in `null_semantics`.
    assert sql_vocabulary(harness) == address.NULL_SENTINELS

    parser = address.RegexV1Parser()
    for case in ADDRESS_CASES:
        row = harness.eval_macro("address_parse", case.line)[0]
        parsed = parser.parse(case.line)
        macro_components = tuple(row[column] for column in PARSED_COLUMNS)
        oracle_components = (parsed.addr_number, parsed.addr_street, parsed.addr_unit)
        assert macro_components == oracle_components, (
            f"macro and oracle disagree on {case.line!r} ({case.why})"
        )
        # Both against the committed expectation, so a drift that moved BOTH
        # implementations the same way is still a failure.
        assert macro_components == case.components, f"{case.line!r} ({case.why})"


def test_emits_six_addr_aliases(harness: MacroHarness) -> None:
    """AC4: exactly the six S5 aliases, and exactly six projections."""
    rendered = harness.render_macro("address_parse", "line", "city", "region", "postal")
    projections = split_projections(rendered)
    assert len(projections) == len(ADDR_COLUMNS)

    aliases = []
    for projection in projections:
        match = ALIAS_RE.search(projection)
        assert match is not None, f"projection carries no alias: {projection}"
        aliases.append(match.group(1))
    assert tuple(aliases) == ADDR_COLUMNS

    # The harness refuses ATTACH, so this also records that the macro reaches for
    # no warehouse; and every alias is a column S5 declares, not a new one.
    assert "attach" not in rendered.lower()


def test_city_region_postal_are_normalized_passthroughs(harness: MacroHarness) -> None:
    """AC4: the three mapped columns come from their own source columns only."""
    rendered = harness.render_macro("address_parse", "line", "city", "region", "postal")
    statement = (
        f"select {rendered} from (select cast(? as varchar) as line, "
        "cast(? as varchar) as city, cast(? as varchar) as region, "
        "cast(? as varchar) as postal) as input"
    )

    def evaluate(line: str | None) -> dict[str, str | None]:
        cursor = harness.execute(statement, [line, "  Springfield ", "OR", " 97401 "])
        columns = [column[0] for column in cursor.description or ()]
        return dict(zip(columns, cursor.fetchone(), strict=True))

    first = evaluate("123 Main St Apt 4B")
    assert first["addr_city"] == "springfield"
    assert first["addr_region"] == "or"
    assert first["addr_postal"] == "97401"
    # The postal code is NOT reformatted: S4.2 gives it `lowercase_trim` and
    # sentinel handling and nothing else, so no ZIP+4 collapsing sneaks in here.
    assert evaluate("1 A St")["addr_postal"] == "97401"

    # An `address_line` change moves the parsed components and nothing else.
    second = evaluate("456 Oak Ave")
    assert (second["addr_number"], second["addr_street"]) == ("456", "oak ave")
    assert {key: second[key] for key in ("addr_city", "addr_region", "addr_postal")} == {
        key: first[key] for key in ("addr_city", "addr_region", "addr_postal")
    }

    # `null_semantics` applies to the mapped columns too: a sentinel in the source
    # column is absence, not the literal string `unknown` reaching the matcher.
    cursor = harness.execute(statement, ["123 Main St", "UNKNOWN", "  ", None])
    columns = [column[0] for column in cursor.description or ()]
    sentinel_row = dict(zip(columns, cursor.fetchone(), strict=True))
    assert sentinel_row["addr_city"] is None
    assert sentinel_row["addr_region"] is None
    assert sentinel_row["addr_postal"] is None
    # ... and the address line beside them still parsed.
    assert sentinel_row["addr_number"] == "123"
