"""The `AddressParser` interface and its v1 implementation (DesignDoc.md S4.2, S6).

S4.2 puts `address_parse` "behind the `AddressParser` interface, versioned by
`versions.address_parser_version`", which makes three things normative here:

* **The interface is the seam.** :class:`AddressParser` is what a libpostal
  container would implement later; S4.2 says such a swap costs no model change,
  and it only costs none if nothing outside this module knows which parser it has.
* **The version selects the parser.** :func:`get_address_parser` is the only
  selector, and an unknown version is a *config* error (S4.0 exit ``2``), not a
  fallback to v1. `address_parser_version` is a `versions:` field (S6.1 V13) and
  therefore inside `config_hash` (S5.2), so a parser swap moves the hash and the
  S4.0 config-drift guard catches it -- but only because the selected parser's
  version is asserted to be the configured one rather than assumed.
* **An unparsable line yields NULL components, never a guess** (S4.2). The fixture
  generator emits only patterns this parser handles, so a partial parse would be a
  silent corpus corruption: a box number in `addr_number` still compares equal to
  a house number.

:class:`RegexV1Parser` is also the ORACLE for `dbt/macros/std/address_parse.sql`.
The macro is the implementation the pipeline runs; this class is the one the unit
layer can reason about, and the two are two implementations of one grammar. The
three patterns below are therefore written to be valid in both RE2 (DuckDB) and
:mod:`re`, and `tests/unit/dbt/test_address.py` asserts that the macro renders the
identical strings and that both sides agree on every committed case. That is the
T-BLK-1 shape applied to standardization -- the drift is only visible if something
compares the two.

The module is pure: no connection, no I/O, and no import from anywhere under
``src/er`` except the error taxonomy whose exit code S4.0 pins.
"""

from __future__ import annotations

import re
import unicodedata
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from types import MappingProxyType
from typing import Final, Protocol, runtime_checkable

from er.config.schema import Config
from er.errors import ConfigError

__all__ = [
    "ADDRESS_NUMBER_PATTERN",
    "ADDRESS_PO_BOX_PATTERN",
    "ADDRESS_UNIT_PATTERN",
    "NO_COMPONENTS",
    "NULL_SENTINELS",
    "REGEX_V1_VERSION",
    "AddressComponents",
    "AddressParser",
    "RegexV1Parser",
    "get_address_parser",
]

#: The trailing unit phrase: `<keyword> <designator>`, or `#<designator>`. Group 2
#: is the unit; the leading ``(^| )`` is part of the match so removing the phrase
#: takes its separator with it. Anchored at the end, so a line carries at most one
#: unit and re-parsing a rendered line finds the same one.
ADDRESS_UNIT_PATTERN: Final = (
    "(^| )((apt|apartment|unit|ste|suite|fl|floor|rm|room) [a-z0-9-]+|#[a-z0-9-]+)$"
)

#: A PO box in every spelling the punctuation strip leaves. It is REJECTED rather
#: than parsed: a box has no number/street/unit decomposition, and inventing one
#: would put a box number where a comparison level reads a house number.
ADDRESS_PO_BOX_PATTERN: Final = "^(p ?o|post office) box( .*)?$"

#: The house number: digits and at most one letter, then a boundary. `221b` is a
#: number; `5th` is not, because the boundary after the optional letter fails.
ADDRESS_NUMBER_PATTERN: Final = "^([0-9]+[a-z]?)( |$)"

#: The S4.2 sentinel vocabulary. `dbt/macros/std/null_semantics.sql::NULL_SENTINELS`
#: is the single SQL-side declaration; this tuple is the oracle's spelling of the
#: same list and `tests/unit/dbt/test_address.py` asserts the two are equal, so the
#: vocabulary still has one definition a change has to pass through.
NULL_SENTINELS: Final[tuple[str, ...]] = ("", "null", "n/a", "-", "unknown")

#: The value of `versions.address_parser_version` that selects :class:`RegexV1Parser`.
REGEX_V1_VERSION: Final = "1"

_UNIT_RE: Final = re.compile(ADDRESS_UNIT_PATTERN)
_PO_BOX_RE: Final = re.compile(ADDRESS_PO_BOX_PATTERN)
_NUMBER_RE: Final = re.compile(ADDRESS_NUMBER_PATTERN)

# DuckDB's `trim` removes spaces and not other whitespace, and RE2's `[[:space:]]`
# is the ASCII class. Both are spelled out here rather than reached for through
# `str.strip()` and `\s`, which are wider in Python and would make the oracle
# disagree with the macro on an input carrying U+00A0.
_SPACE: Final = " "
_WHITESPACE_RE: Final = re.compile("[ \t\n\v\f\r]+")
_PUNCTUATION_RE: Final = re.compile("[.,]")


@dataclass(frozen=True, slots=True)
class AddressComponents:
    """The three components S4.2 parses out of the single `address_line`.

    `addr_city`, `addr_region` and `addr_postal` are deliberately absent: S6 maps
    them directly from their own source columns, so they are not a parse result
    and a parser that returned them would invite a caller to overwrite a mapped
    value with a guessed one.

    All three fields are NULL together or the parse succeeded (S4.2): a line the
    grammar does not recognise has no street, and a street is what makes the other
    two components mean anything.
    """

    addr_number: str | None
    addr_street: str | None
    addr_unit: str | None


#: What an unrecognised line parses to. Named because "all three NULL" is a stated
#: outcome of the grammar (S4.2) rather than an incidental empty result.
NO_COMPONENTS: Final = AddressComponents(None, None, None)


@runtime_checkable
class AddressParser(Protocol):
    """Componentizes one `address_line` into the three parsed S5 `addr_*` values.

    ``version`` is a read-only member on purpose: it is the identity the S4.0
    start-up assertion compares against `versions.address_parser_version`, and a
    parser that could be re-versioned after construction would make that check
    describe the moment it ran rather than the parser.
    """

    @property
    def version(self) -> str:
        """The `versions.address_parser_version` value this parser implements."""

    def parse(self, address_line: str | None) -> AddressComponents:
        """Return the components of ``address_line``; all NULL if unrecognised."""


class RegexV1Parser:
    """The v1 grammar, in Python. The oracle for `dbt/macros/std/address_parse.sql`.

    Stateless and therefore shareable: it holds no compiled state a caller could
    invalidate and no configuration beyond the version it answers for.
    """

    @property
    def version(self) -> str:
        return REGEX_V1_VERSION

    def parse(self, address_line: str | None) -> AddressComponents:
        line = _normalize(address_line)
        if line is None:
            return NO_COMPONENTS

        unit_match = _UNIT_RE.search(line)
        unit = unit_match.group(2) if unit_match is not None else None
        # Unconditional, like the macro's `regexp_replace`: a line with no unit
        # phrase is left alone by the substitution rather than by a branch, so the
        # two implementations cannot differ on which case they took.
        rest = _UNIT_RE.sub("", line, count=1).strip(_SPACE)

        number_match = _NUMBER_RE.match(rest)
        number = number_match.group(1) if number_match is not None else None
        street = _NUMBER_RE.sub("", rest, count=1).strip(_SPACE)
        if not street:
            # No street: a bare number, or a unit with nothing to attach it to.
            # S4.2 makes this all-NULL rather than a partial parse.
            return NO_COMPONENTS
        return AddressComponents(addr_number=number, addr_street=street, addr_unit=unit)


def _normalize(address_line: str | None) -> str | None:
    """Apply the macro's normalization chain, in the macro's order.

    `lowercase_trim` -> punctuation -> whitespace -> `null_semantics` -> PO box.
    `null_semantics` runs LAST for the reason `name_norm` states: applied earlier,
    the punctuation strip could manufacture a sentinel out of a value the first
    pass let through, and re-parsing would then answer differently.
    """
    if address_line is None:
        return None
    # `lowercase_trim`: NFC, trim, lower, NFC again -- lowering can compose a
    # sequence that was not composable before it, so a single leading NFC does not
    # make the output NFC.
    text = unicodedata.normalize("NFC", address_line)
    text = unicodedata.normalize("NFC", text.strip(_SPACE).lower())
    if not text:
        return None

    text = _PUNCTUATION_RE.sub("", text)
    text = _WHITESPACE_RE.sub(_SPACE, text).strip(_SPACE)
    if not text or text in NULL_SENTINELS:
        return None
    if _PO_BOX_RE.fullmatch(text) is not None:
        return None
    return text


#: Every parser version this build knows, keyed by the `versions:` value that
#: selects it. Adding a v2 means adding a row here and an S2.1 pin if it brings a
#: dependency (S4.2 puts `usaddress`/`libpostal` explicitly out of v1).
_PARSERS: Final[Mapping[str, Callable[[], AddressParser]]] = MappingProxyType(
    {REGEX_V1_VERSION: RegexV1Parser}
)


def get_address_parser(config: Config) -> AddressParser:
    """Return the parser `versions.address_parser_version` selects.

    An unknown version raises :class:`~er.errors.ConfigError` -- S4.0 exit ``2``,
    the code a bad config document gets -- rather than falling back to v1. A silent
    fallback would standardize the whole corpus under a parser the document did not
    ask for while `config_hash` recorded the one it did.
    """
    requested = config.versions.address_parser_version
    factory = _PARSERS.get(requested)
    if factory is None:
        known = ", ".join(repr(version) for version in sorted(_PARSERS))
        raise ConfigError(
            f"versions.required: unknown address_parser_version {requested!r}; "
            f"this build implements {known}"
        )
    parser = factory()
    if parser.version != requested:
        # Guards the registry, not the caller: a parser filed under a version it
        # does not answer for would put `config_hash` and the corpus out of step
        # in exactly the way selecting by version exists to prevent.
        raise ConfigError(
            f"versions.required: address_parser_version {requested!r} selects a parser "
            f"reporting version {parser.version!r}"
        )
    return parser
