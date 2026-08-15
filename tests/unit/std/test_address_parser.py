"""`RegexV1Parser` and parser selection (DesignDoc.md S4.2, S6, S6.1 V13; S8.4).

The oracle half of the address grammar. `tests/unit/dbt/test_address.py` is the
other half and is where the two implementations are compared; this file pins what
the grammar IS, so a parity failure there can be read as "the macro moved" or "the
oracle moved" rather than only as "they disagree".

Three properties carry the weight:

* every committed case parses to its committed components,
* an unrecognised line yields all three components NULL -- never a partial parse,
  because a box number sitting in `addr_number` still compares equal to a house
  number and would merge two entities that share nothing, and
* the parse is idempotent on a rendered line, which is what lets a corpus be
  re-standardized without drifting (S4.2).

Nothing here opens a connection, spawns dbt or reads the lake (AC7).
"""

from __future__ import annotations

import unicodedata
from pathlib import Path

import pytest
from unit.std.address_cases import ADDRESS_CASES, AddressCase

from er.config.loader import load_config
from er.config.schema import Config, Versions
from er.errors import ConfigError, ExitCode
from er.std.address import AddressParser, RegexV1Parser, get_address_parser

#: `configs/test.yaml` is the document S6 says the fixtures and CI use verbatim.
TEST_CONFIG_PATH = Path(__file__).resolve().parents[3] / "configs" / "test.yaml"


def render(case: AddressCase) -> str:
    """The `'<addr_number> <addr_street> <addr_unit>'` line AC6 re-parses.

    NULL components contribute nothing rather than an empty token: the rendering
    is what a downstream consumer would write back, and a consumer does not emit
    a placeholder for a unit the address does not have.
    """
    return " ".join(part for part in case.components if part is not None)


def test_regex_v1_parses_the_committed_cases() -> None:
    """Every row of the shared table, field for field."""
    parser = RegexV1Parser()
    assert parser.version == "1"

    for case in ADDRESS_CASES:
        parsed = parser.parse(case.line)
        assert (parsed.addr_number, parsed.addr_street, parsed.addr_unit) == case.components, (
            f"{case.line!r} ({case.why})"
        )

    # AC3, called out rather than left to be found among the rows above.
    apartment = parser.parse("123 Main St Apt 4B")
    assert apartment.addr_number == "123"
    assert apartment.addr_street == "main st"
    assert apartment.addr_unit == "apt 4b"
    assert parser.parse("123 Main St").addr_unit is None

    # AC2's NFC/NFD pair is two spellings, not one string asserted twice.
    spellings = [case.line for case in ADDRESS_CASES if case.addr_street == "cañon rd"]
    assert len(spellings) == 2
    assert spellings[0] != spellings[1]
    assert unicodedata.normalize("NFC", str(spellings[1])) == spellings[0]

    # The table covers at least the twenty inputs AC2 asks for, and every input is
    # distinct -- a duplicated row would inflate the count without covering a rule.
    assert len(ADDRESS_CASES) >= 20
    assert len({case.line for case in ADDRESS_CASES}) == len(ADDRESS_CASES)


def test_unparsable_line_yields_all_null_components() -> None:
    """AC3: no partial parse. All three components are NULL together, or none are."""
    parser = RegexV1Parser()
    unparsable = (
        "PO Box 7",  # a box has no number/street/unit decomposition
        "P.O. Box 7",
        "Post Office Box 12",
        "Apt 4B",  # a unit with nothing to attach it to
        "#3",
        "123",  # a bare number: the street is what makes the rest mean anything
        "",
        "   ",
        "unknown",
        None,
    )
    for line in unparsable:
        parsed = parser.parse(line)
        assert (parsed.addr_number, parsed.addr_street, parsed.addr_unit) == (None, None, None), (
            f"{line!r} produced a partial parse"
        )

    # The failure this asserts against: `PO Box 7` yielding number `7`, or
    # `123` yielding street `123`, either of which is evidence the corpus does
    # not actually carry.
    assert parser.parse("PO Box 7").addr_number is None
    assert parser.parse("123").addr_street is None

    # Every all-NULL row in the committed table is one of these shapes, so the
    # table cannot grow a silently-unparsable address nobody meant to add.
    for case in ADDRESS_CASES:
        if case.components == (None, None, None):
            assert case.line in unparsable or case.line in ("N/A", "-"), (
                f"{case.line!r} parses to nothing but is not a documented rejection"
            )


def test_get_address_parser_selects_by_version_and_rejects_unknown() -> None:
    """AC5: the version selects the parser, and an unknown one exits 2."""
    config = load_config(TEST_CONFIG_PATH)
    assert config.versions.address_parser_version == "1"

    parser = get_address_parser(config)
    assert isinstance(parser, RegexV1Parser)
    assert isinstance(parser, AddressParser)
    # The assertion that gives `address_parser_version` its role (S5.2, S6.1 V13):
    # the parser in hand answers for the version `config_hash` covers.
    assert parser.version == config.versions.address_parser_version

    unknown: Config = config.model_copy(
        update={
            "versions": Versions(
                std_version=config.versions.std_version,
                survivorship_version=config.versions.survivorship_version,
                address_parser_version="2",
            )
        }
    )
    with pytest.raises(ConfigError) as raised:
        get_address_parser(unknown)
    # S4.0: a config error is exit 2. Asserted through the taxonomy rather than as
    # a literal, so the code and the class cannot disagree.
    assert raised.value.code == int(ExitCode.CONFIG)
    assert "address_parser_version" in str(raised.value)


def test_parse_is_idempotent_on_a_rendered_line() -> None:
    """AC6: re-parsing `'<number> <street> <unit>'` reproduces the components."""
    parser = RegexV1Parser()
    for case in ADDRESS_CASES:
        once = parser.parse(case.line)
        reparsed = parser.parse(render(case))
        assert reparsed == once, f"{case.line!r} does not survive a round trip ({case.why})"

    # The rendering is a real second spelling for at least the unit cases, so the
    # property is not passing because every rendered line equals its input.
    rendered = {render(case) for case in ADDRESS_CASES}
    assert rendered != {str(case.line) for case in ADDRESS_CASES}
