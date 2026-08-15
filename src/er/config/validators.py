"""The cross-field and referential config validators V1-V8 (DesignDoc.md S6.1, S5.0).

The single-block validators live on the models in :mod:`er.config.schema`; the eight
here cannot, because each one reads either two blocks at once or a column list that
belongs to the data model. Three properties are normative and stated nowhere else:

* Every rejection carries the *literal* S6.1 failure message key, so a failure names
  the rule that rejected the document rather than merely failing. :data:`FAILURE_KEYS`
  is that column of the S6.1 table, transcribed.
* The column vocabularies are imported from :mod:`er.lake.columns` and never restated.
  V2 exists precisely to make the ``survivorship:`` key set and
  ``GOLDEN_SURVIVABLE_COLUMNS`` one fact (S5.0), which a second copy here would undo.
* Nothing on this path imports an engine or opens a connection: S4.0 validates before
  any lake connection exists, so ``duckdb`` MUST still be absent from ``sys.modules``
  after a document has been rejected.

:func:`validate_cross_fields` runs after the per-block validators (V9-V16) and *before*
:func:`er.config.schema.normalize`. The order is load-bearing in both directions: V3
would reject the terminal ``record_key ASC`` that normalization appends, and V8 reads
the ``null`` token that normalization removes.
"""

from __future__ import annotations

import re
from collections.abc import Callable, Iterable, Mapping
from types import MappingProxyType
from typing import Final

from er.config.schema import TERMINAL_SURVIVORSHIP_RULE, Config
from er.lake.columns import GOLDEN_SURVIVABLE_COLUMNS, STD_RECORD_COLUMNS

__all__ = [
    "ADDRESS_COLUMNS",
    "ADDRESS_KEY",
    "COMPARISON_LEVEL_TOKENS",
    "FAILURE_KEYS",
    "SURVIVORSHIP_KEYS",
    "SURVIVORSHIP_RULES",
    "CrossFieldError",
    "expand_survivorship_keys",
    "validate_cross_fields",
]

#: The failure message key of each S6.1 row this module owns, transcribed from the
#: table. Every raise site names its row rather than spelling the key inline, so a
#: typo in a key is impossible and the table is greppable from one place.
FAILURE_KEYS: Final[Mapping[str, str]] = MappingProxyType(
    {
        "V1": "thresholds.ordering",
        "V2": "survivorship.keyset",
        "V3": "survivorship.unknown_rule",
        "V4": "survivorship.validated_missing_column",
        "V5": "survivorship.not_separating",
        "V6": "columns.unknown",
        "V7": "blocking.duplicate_key_type",
        "V8": "comparisons.unknown_level",
    }
)

_SOURCE_PRIORITY = "source_priority"
_VALIDATED = "validated"

#: The S6.1 V3 rule vocabulary, in the order S4.6 tabulates the `ORDER BY` fragments.
#: The mandatory terminal `record_key ASC` is NOT a member: normalization appends it
#: after validation, and a document that spells it out is naming a rule S6 does not.
SURVIVORSHIP_RULES: Final[tuple[str, ...]] = (
    _SOURCE_PRIORITY,
    "recency",
    "frequency",
    "completeness",
    _VALIDATED,
)

# `source_priority` is the one rule of S4.6 that cannot separate two records from the
# same source -- both carry that source's `priority_rank` -- so a chain built only from
# it leaves every contest to the terminal `record_key ASC` (V5).
_SEPARATING_RULES: Final[frozenset[str]] = frozenset(SURVIVORSHIP_RULES) - {_SOURCE_PRIORITY}

#: The composite `survivorship:` key of S6. It is the only key that is not itself a
#: `golden_records` column, and S4.6 requires all six of its columns to come from one
#: winning record, so they win or lose together and expand together.
ADDRESS_KEY = "address"

#: Derived, never listed: the `addr_*` members of the imported survivable tuple.
ADDRESS_COLUMNS: Final[tuple[str, ...]] = tuple(
    column for column in GOLDEN_SURVIVABLE_COLUMNS if column.startswith("addr_")
)

#: The `survivorship:` key set S6.1 V2 requires, as a consequence of S5.0 rather than
#: as a second listing of it: every survivable golden column, with the six address
#: ones collapsed into `address`.
SURVIVORSHIP_KEYS: Final[frozenset[str]] = (
    frozenset(GOLDEN_SURVIVABLE_COLUMNS) - frozenset(ADDRESS_COLUMNS)
) | {ADDRESS_KEY}

_NULL_TOKEN = "null"
_VARIANT_MATCH = "variant_match"
_JARO_WINKLER_PREFIX = "jaro_winkler:"

#: The non-parametric comparison level tokens of S4.3.1. `jaro_winkler:<T>` is the one
#: token carrying an argument and is checked separately. `null` is a member because
#: validators run before normalization removes it; there is deliberately no `phonetic`.
COMPARISON_LEVEL_TOKENS: Final[frozenset[str]] = frozenset(
    {"exact", _NULL_TOKEN, "username_exact", _VARIANT_MATCH, "dob_same_year_month"}
)

#: `variant_match` maps to `ArrayIntersectLevel('name_variants', ...)` whatever column
#: it compares (S4.3.1), so the token carries its own referential requirement.
_VARIANTS_COLUMN = "name_variants"

# The `<attr>_valid` column the `validated` rule of S4.6 orders by. The default is the
# attribute's own name, and S6.1 V4 spells the pairing out because one attribute breaks
# it: `phone_e164`'s flag is `phone_valid`, named after the canonical attribute of S6
# rather than after the standardized column the chain survives.
_VALIDATED_COLUMNS: Final[Mapping[str, str]] = MappingProxyType({"phone_e164": "phone_valid"})

# Bare words a blocking `expr` may contain that are not column references. The set is
# deliberately small: an identifier this scan does not recognise is treated as a column
# and therefore rejected by V6, which is the safe direction for a referential check --
# the failure mode of the other direction is a blocking rule that compiles to SQL
# referencing a column `int_std_records` does not have.
_SQL_WORDS: Final[frozenset[str]] = frozenset(
    {
        "and",
        "as",
        "between",
        "case",
        "distinct",
        "else",
        "end",
        "false",
        "in",
        "is",
        "like",
        "not",
        _NULL_TOKEN,
        "or",
        "then",
        "true",
        "when",
    }
)

_STRING_LITERAL_RE = re.compile(r"'[^']*'")
_IDENTIFIER_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


class CrossFieldError(Exception):
    """One V1-V8 rejection: its S6.1 message key, its detail, and where it happened.

    The loader turns it into a ``ConfigValidationError`` carrying exit code 2 and the
    JSON pointer S4.0 prints; nothing else catches it.
    """

    def __init__(self, key: str, detail: str, loc: tuple[str | int, ...]) -> None:
        # The message leads with the key, as every validator in `schema` does, so the
        # loader reads both the same way whichever layer rejected the document.
        super().__init__(f"{key}: {detail}")
        self.key = key
        self.message = f"{key}: {detail}"
        self.loc = loc


def _fail(row: str, loc: tuple[str | int, ...], detail: str) -> CrossFieldError:
    """The error for one S6.1 row, so no raise site spells a message key itself."""
    return CrossFieldError(FAILURE_KEYS[row], detail, loc)


def expand_survivorship_keys(keys: Iterable[str]) -> frozenset[str]:
    """The `survivorship:` keys as `golden_records` columns (S6.1 V2, S5.0).

    ``address`` expands to the six ``addr_*`` members of ``GOLDEN_SURVIVABLE_COLUMNS``;
    every other key is already a column and maps to itself. A key S6 does not name is
    returned unchanged rather than dropped, so a caller comparing the result against
    the survivable set still sees it in the difference.
    """
    expanded: set[str] = set()
    for key in keys:
        if key == ADDRESS_KEY:
            expanded.update(ADDRESS_COLUMNS)
        else:
            expanded.add(key)
    return frozenset(expanded)


def _expression_columns(expr: str) -> list[str]:
    """The column names a blocking `expr` references, in the order they appear."""
    # String literals first: the separator in `... || '|' || ...` is not an identifier,
    # but a literal containing a word would otherwise read as one.
    scrubbed = _STRING_LITERAL_RE.sub(" ", expr)
    columns: list[str] = []
    for match in _IDENTIFIER_RE.finditer(scrubbed):
        word = match.group()
        if scrubbed[match.end() :].lstrip().startswith("("):
            continue  # a function call -- `substr(family_name,1,4)` names one column
        if word.lower() in _SQL_WORDS:
            continue
        columns.append(word)
    return columns


def _v1_threshold_ordering(config: Config) -> None:
    thresholds = config.thresholds
    if not 0 < thresholds.review_low < thresholds.auto_merge <= 1:
        raise _fail(
            "V1",
            ("thresholds",),
            f"0 < review_low < auto_merge <= 1 is required, got review_low="
            f"{thresholds.review_low} and auto_merge={thresholds.auto_merge}; equal "
            f"thresholds leave the half-open gray band of S4.3 empty",
        )


def _v2_survivorship_keyset(config: Config) -> None:
    keys = frozenset(config.survivorship)
    if keys == SURVIVORSHIP_KEYS:
        return
    raise _fail(
        "V2",
        ("survivorship",),
        f"the survivorship key set must equal {sorted(SURVIVORSHIP_KEYS)}, which is the "
        f"survivable column set of golden_records with the address columns as "
        f"{ADDRESS_KEY!r}: missing {sorted(SURVIVORSHIP_KEYS - keys)}, unexpected "
        f"{sorted(keys - SURVIVORSHIP_KEYS)} (these keys expand to "
        f"{sorted(expand_survivorship_keys(keys))})",
    )


def _v3_known_rules(config: Config) -> None:
    for attribute, chain in config.survivorship.items():
        for rule in chain:
            if rule not in SURVIVORSHIP_RULES:
                raise _fail(
                    "V3",
                    ("survivorship", attribute),
                    f"{rule!r} is not a survivorship rule; S4.6 dispatches only "
                    f"{list(SURVIVORSHIP_RULES)}",
                )


def _v4_validated_has_column(config: Config) -> None:
    for attribute, chain in config.survivorship.items():
        if _VALIDATED not in chain:
            continue
        column = _VALIDATED_COLUMNS.get(attribute, f"{attribute}_valid")
        if column not in STD_RECORD_COLUMNS:
            raise _fail(
                "V4",
                ("survivorship", attribute),
                f"the {_VALIDATED!r} rule orders by {column!r}, which int_std_records "
                f"does not carry; only email and phone_e164 have a _valid column",
            )


def _v5_chain_separates(config: Config) -> None:
    for attribute, chain in config.survivorship.items():
        if not _SEPARATING_RULES.intersection(chain):
            raise _fail(
                "V5",
                ("survivorship", attribute),
                f"chain {chain} holds no rule able to separate two records from the same "
                f"source, so the terminal {TERMINAL_SURVIVORSHIP_RULE!r} would decide "
                f"every contest; add one of {sorted(_SEPARATING_RULES)}",
            )


def _v6_referential_columns(config: Config) -> None:
    # One pass collecting every column the document requires, each with the location
    # that required it, then one membership test: `name_variants` is required by a
    # level rather than by a key, and this is what lets it be checked the same way.
    required: dict[str, tuple[str | int, ...]] = {}
    for index, rule in enumerate(config.blocking):
        for column in _expression_columns(rule.expr):
            required.setdefault(column, ("blocking", index, "expr"))
    for column, comparison in config.comparisons.items():
        required.setdefault(column, ("comparisons", column))
        if _VARIANT_MATCH in comparison.levels:
            required.setdefault(_VARIANTS_COLUMN, ("comparisons", column, "levels"))

    known = frozenset(STD_RECORD_COLUMNS)
    for column, loc in required.items():
        if column not in known:
            raise _fail(
                "V6",
                loc,
                f"{column!r} is not a column of int_std_records (S5); the relation has "
                f"{list(STD_RECORD_COLUMNS)}",
            )


def _v7_unique_key_types(config: Config) -> None:
    seen: set[str] = set()
    for index, rule in enumerate(config.blocking):
        if rule.key_type in seen:
            raise _fail(
                "V7",
                ("blocking", index, "key_type"),
                f"key_type {rule.key_type!r} is used by more than one blocking rule; "
                f"int_blocking_keys is keyed on (key_type, key_value, record_key)",
            )
        seen.add(rule.key_type)


def _is_level_token(token: str) -> bool:
    """Whether one pre-normalization level token is in the S4.3.1 vocabulary."""
    if token in COMPARISON_LEVEL_TOKENS:
        return True
    if not token.startswith(_JARO_WINKLER_PREFIX):
        return False
    try:
        threshold = float(token.removeprefix(_JARO_WINKLER_PREFIX))
    except ValueError:
        return False
    # `nan` and `inf` parse but fail this comparison, which is the intent: the
    # threshold is a similarity in (0, 1].
    return 0 < threshold <= 1


def _v8_level_tokens(config: Config) -> None:
    for column, comparison in config.comparisons.items():
        for level in comparison.levels:
            # YAML's `null` parses to None and is still present here, because
            # normalization runs after validation.
            token = _NULL_TOKEN if level is None else level
            if not _is_level_token(token):
                raise _fail(
                    "V8",
                    ("comparisons", column, "levels"),
                    f"{level!r} is not a comparison level of S4.3.1; the vocabulary is "
                    f"{sorted(COMPARISON_LEVEL_TOKENS)} plus "
                    f"{_JARO_WINKLER_PREFIX}<T> with 0 < T <= 1",
                )


# In S6.1 table order, which is also the order a reader of the spec expects a document
# to be rejected in.
_VALIDATORS: Final[tuple[Callable[[Config], None], ...]] = (
    _v1_threshold_ordering,
    _v2_survivorship_keyset,
    _v3_known_rules,
    _v4_validated_has_column,
    _v5_chain_separates,
    _v6_referential_columns,
    _v7_unique_key_types,
    _v8_level_tokens,
)


def validate_cross_fields(config: Config) -> None:
    """Run S6.1 V1-V8 against a parsed, not yet normalized document.

    Args:
        config: a document that has already passed the per-block validators V9-V16,
            i.e. the output of ``Config.model_validate`` and NOT of
            :func:`er.config.schema.normalize`.

    Raises:
        CrossFieldError: the first row that rejects the document, carrying that row's
            literal S6.1 failure message key.
    """
    for validator in _VALIDATORS:
        validator(config)
