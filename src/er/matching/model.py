"""The S4.2 blocking generator and the S4.3.1 settings builder (DesignDoc.md S4.2, S4.3).

Two pure functions live here, and they are the only two things that turn the S6
document into Splink inputs: :func:`blocking_rules_from_config` for candidate
generation and :func:`build_settings` for the comparison model. Neither opens a
connection, imports `duckdb` or constructs a `Linker` — that is ER-049's — so both
are callable from a bare unit test and from the CLI before the lake is attached.

`configs/*.yaml` `blocking:` is the single source of truth for candidate generation,
and :func:`blocking_rules_from_config` is the only bridge from it to the two things
that consume it: the dbt var the macro-generated `int_blocking_keys` renders one
`UNION ALL` branch per entry from (ER-047), and the Splink `block_on` rules S4.3.4
hands to both inference passes. Three properties are normative and live only here:

* The `expr` string reaches both consumers **unmodified**. The dbt branch embeds it
  and Splink receives `block_on('<expr>')` on the byte-identical string, which is
  exactly the precondition T-BLK-1 checks; a normalisation, requoting or whitespace
  fix on either path would make the two candidate sets diverge for a reason no test
  could attribute to it. S4.2 locates that byte-identity at the `block_on` *input*
  and nowhere else: `block_on` re-renders through sqlglot and qualifies every column
  with `l.` / `r.` inside the expression, so the raw `expr` is deliberately not a
  substring of the rendered rule.
* Rule order equals config order. Splink deduplicates candidates across rules by
  excluding the pairs preceding rules already produced, so the position of a rule
  decides which rule a pair is attributed to.
* The NULL/empty policy is rendered once, here, and consumed verbatim by the macro:
  a NULL or empty key value is never emitted and never blocks, on either side.

`int_blocking_keys` is never read back: it is not an input to scoring (S4.3.4), and
this module is the only thing that turns the config into blocking.

The settings half maps each `comparisons.<attr>.levels` token to exactly one Splink
construct through :data:`LEVEL_TOKENS`, and two of its emission rules are normative
and easy to lose:

* `NullLevel` is ALWAYS first and `ElseLevel()` ALWAYS last, for every comparison,
  whatever the config `levels` list holds. Without the else level Splink renders a
  `CASE … END` with no `ELSE`, so a pair matching no level gets gamma NULL and a NULL
  match weight — silently, and only for the pairs that score lowest. The `null` token
  is stripped from `levels` by S6.1 normalization, so the first level cannot be driven
  by the list either; it is unconditional.
* `tf: true` reaches the exact-match level of the named column and nothing else.
"""

from __future__ import annotations

import json
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from types import MappingProxyType
from typing import Any, Final

from splink import SettingsCreator, block_on
from splink.comparison_level_library import (
    ArrayIntersectLevel,
    CustomLevel,
    ElseLevel,
    ExactMatchLevel,
    JaroWinklerLevel,
    NullLevel,
)
from splink.comparison_library import CustomComparison
from splink.internals.blocking_rule_creator import BlockingRuleCreator
from splink.internals.comparison_creator import ComparisonCreator
from splink.internals.comparison_level_creator import ComparisonLevelCreator

from er.config.schema import ComparisonSpec, Config
from er.config.validators import FAILURE_KEYS, _expression_columns
from er.errors import ConfigError
from er.lake.columns import STD_RECORD_COLUMNS

__all__ = [
    "BLOCKING_DBT_VAR",
    "DOB_SAME_YEAR_MONTH_SQL",
    "LEVEL_TOKENS",
    "LINK_TYPE",
    "NAME_VARIANTS_COLUMN",
    "NULL_EMPTY_PREDICATE",
    "SQL_DIALECT",
    "UNIQUE_ID_COLUMN",
    "USERNAME_EXACT_SQL",
    "BlockingKeySpec",
    "BlockingPayload",
    "blocking_rules_from_config",
    "build_settings",
    "settings_json",
]

#: The name the CLI passes the payload to dbt under, merged into
#: :func:`er.dbt_runner.render_dbt_vars`'s `extra`. It is the S6 block's own name
#: because the var IS that block, rendered: a macro reading `var('blocking')` names
#: the config key a reader has to go and look at, and any other spelling would be a
#: second name for one fact.
BLOCKING_DBT_VAR: Final = "blocking"

#: The S4.2 NULL/empty predicate, with `{expr}` as its single substitution point.
#: Emitted once here and consumed verbatim by ER-047's macro. Both halves are
#: load-bearing: `is not null` alone would let the empty string block every record
#: whose key expression concatenates a missing component, and Splink's `block_on`
#: never joins on NULL, so this is what makes the dbt side agree with it.
NULL_EMPTY_PREDICATE: Final = "{expr} is not null and {expr} <> ''"

#: The dbt var payload: a JSON-native list of `{key_type, expr, where}` objects in
#: config order. Deliberately plain `str`-to-`str` mappings — it travels through
#: `json.dumps` into an argv element (S4.6), so nothing in it may be a type only
#: Python can render.
BlockingPayload = list[dict[str, str]]


@dataclass(frozen=True)
class BlockingKeySpec:
    """One `blocking:` entry in the form both consumers need (S4.2)."""

    #: The `int_blocking_keys.key_type` literal this entry emits.
    key_type: str
    #: The key expression, byte-identical to the config value.
    expr: str
    #: :data:`NULL_EMPTY_PREDICATE` with :attr:`expr` substituted.
    where: str

    @classmethod
    def from_rule(cls, key_type: str, expr: str) -> BlockingKeySpec:
        """The spec for one config entry, rendering the S4.2 predicate around `expr`."""
        return cls(key_type=key_type, expr=expr, where=NULL_EMPTY_PREDICATE.format(expr=expr))

    def as_payload(self) -> dict[str, str]:
        """This entry as the dbt var carries it."""
        return {"key_type": self.key_type, "expr": self.expr, "where": self.where}


def _reject_duplicate_key_types(cfg: Config) -> None:
    """S6.1 V7, re-checked here (`blocking.duplicate_key_type`).

    The loader rejects it too, but a caller may hand this function a `Config` that
    never went through `load_config`, and two entries sharing a `key_type` produce
    two `UNION ALL` branches indistinguishable in `int_blocking_keys`, whose rows are
    keyed on `(key_type, key_value, record_key)`.
    """
    seen: set[str] = set()
    for index, rule in enumerate(cfg.blocking):
        if rule.key_type in seen:
            raise ConfigError(
                f"{FAILURE_KEYS['V7']}: /blocking/{index}/key_type: key_type "
                f"{rule.key_type!r} is used by more than one blocking rule"
            )
        seen.add(rule.key_type)


def _reject_unknown_columns(cfg: Config) -> None:
    """S6.1 V6 over `blocking[].expr` alone (`columns.unknown`).

    The column scan is imported rather than re-derived: a second scanner here could
    accept an `expr` the loader rejects, or the reverse, and one blocking source of
    truth is the whole point of this module. It is private to `er.config.validators`
    because those two rejection sites are its only callers, and this ticket may not
    edit that module to publish it.
    """
    known = frozenset(STD_RECORD_COLUMNS)
    for index, rule in enumerate(cfg.blocking):
        for column in _expression_columns(rule.expr):
            if column not in known:
                raise ConfigError(
                    f"{FAILURE_KEYS['V6']}: /blocking/{index}/expr: {column!r} is not a "
                    f"column of int_std_records (S5); the relation has "
                    f"{list(STD_RECORD_COLUMNS)}"
                )


def _key_specs(cfg: Config) -> list[BlockingKeySpec]:
    """The validated `blocking:` entries, in config order."""
    _reject_duplicate_key_types(cfg)
    _reject_unknown_columns(cfg)
    return [BlockingKeySpec.from_rule(rule.key_type, rule.expr) for rule in cfg.blocking]


def blocking_rules_from_config(
    cfg: Config,
) -> tuple[BlockingPayload, list[BlockingRuleCreator]]:
    """The dbt var payload and the Splink rules for one config (S4.2).

    Args:
        cfg: the validated S6 document. Normalization (S6.1) does not touch
            `blocking:`, so a pre- or post-`normalize` document yields the same
            output.

    Returns:
        `(payload, rules)`, both in config order and both derived from the same
        `expr` strings: the payload for :data:`BLOCKING_DBT_VAR`, and one
        `block_on(expr)` per entry for the two S4.3.4 inference passes.

    Raises:
        ConfigError: two entries share a `key_type` (`blocking.duplicate_key_type`),
            or an `expr` names a column `int_std_records` does not have
            (`columns.unknown`). Both are S4.0 exit `2`.
    """
    specs = _key_specs(cfg)
    # `block_on` over the spec's own `expr`, not over a re-read of the config: the
    # two consumers must be built from one string, and building them from two reads
    # is how they would silently stop being the same string.
    return [spec.as_payload() for spec in specs], [block_on(spec.expr) for spec in specs]


#: The dialect every construct here is rendered for. Splink defers dialecting to
#: render time, so the settings dict does not exist until a dialect is named, and
#: naming a second one anywhere would produce a second settings document.
SQL_DIALECT: Final = "duckdb"

#: `record_key` is the canonical scalar record identity (S5.0) and therefore Splink's
#: `unique_id_column_name`; S4.3 makes passing it this stage's standing obligation.
UNIQUE_ID_COLUMN: Final = "record_key"

#: v1 resolves one corpus against itself. `link_and_dedupe` would require a
#: `source_dataset` column `int_std_records` does not have.
LINK_TYPE: Final = "dedupe_only"

#: The `LIST(VARCHAR)` column `name_norm` emits (S4.2). `variant_match` intersects it
#: whatever attribute the comparison is keyed on, which is orientation-independent
#: only because S4.2 guarantees the normalized `given_name` is element 0 of its own
#: array on every record.
NAME_VARIANTS_COLUMN: Final = "name_variants"

#: The two S4.3.1 tokens with no Splink primitive, as literal SQL. `CustomLevel`
#: passes the string through unrendered, so these are the conditions Splink emits
#: character for character and the spec table is checkable against them directly.
USERNAME_EXACT_SQL: Final = "split_part(email_l,'@',1) = split_part(email_r,'@',1)"
DOB_SAME_YEAR_MONTH_SQL: Final = (
    "date_trunc('month', birth_date_l) = date_trunc('month', birth_date_r)"
)

#: `variant_match` fires on one shared variant: the arrays are nickname expansions of
#: one given name, so a single common element is the whole signal (S4.3.1).
_MIN_INTERSECTION: Final = 1

_NULL_TOKEN: Final = "null"
_JARO_WINKLER_PREFIX: Final = "jaro_winkler:"

#: Builds one comparison level from `(column, token, tf)`. Every builder takes the
#: whole triple even where it reads only part of it, so the table below is a table:
#: the tokens that ignore `tf` are exactly the ones S4.3.1 forbids TF on, and that
#: exclusion is visible in each row rather than asserted somewhere else.
_LevelBuilder = Callable[[str, str, bool], ComparisonLevelCreator]


def _exact_level(column: str, token: str, tf: bool) -> ComparisonLevelCreator:
    # The only row that reads `tf`. Splink renders it as `tf_adjustment_column`, the
    # `.configure(term_frequency_adjustments=True)` of S4.3.1 by another spelling.
    return ExactMatchLevel(column, term_frequency_adjustments=tf)


def _jaro_winkler_level(column: str, token: str, tf: bool) -> ComparisonLevelCreator:
    # The one parametric token: the threshold is in the token, and V8 has already
    # rejected a document whose threshold is outside (0, 1].
    return JaroWinklerLevel(column, float(token.removeprefix(_JARO_WINKLER_PREFIX)))


def _null_level(column: str, token: str, tf: bool) -> ComparisonLevelCreator:
    return NullLevel(column)


def _username_exact_level(column: str, token: str, tf: bool) -> ComparisonLevelCreator:
    return CustomLevel(USERNAME_EXACT_SQL)


def _variant_match_level(column: str, token: str, tf: bool) -> ComparisonLevelCreator:
    return ArrayIntersectLevel(NAME_VARIANTS_COLUMN, min_intersection=_MIN_INTERSECTION)


def _dob_same_year_month_level(column: str, token: str, tf: bool) -> ComparisonLevelCreator:
    return CustomLevel(DOB_SAME_YEAR_MONTH_SQL)


#: The S4.3.1 token table, one row per token and no seventh row. The `jaro_winkler:`
#: row is keyed on its prefix because the threshold rides in the token; every other
#: key is the token itself. The `null` row is not dead: it is what
#: :func:`_comparison` emits as the mandatory first level of every comparison, so
#: `NullLevel` has one construction site and the `null` token has one meaning.
LEVEL_TOKENS: Final[Mapping[str, _LevelBuilder]] = MappingProxyType(
    {
        "exact": _exact_level,
        _JARO_WINKLER_PREFIX: _jaro_winkler_level,
        _NULL_TOKEN: _null_level,
        "username_exact": _username_exact_level,
        "variant_match": _variant_match_level,
        "dob_same_year_month": _dob_same_year_month_level,
    }
)


def _jaro_winkler_threshold(token: str) -> float | None:
    """The `T` of `jaro_winkler:T`, or ``None`` if the token does not carry one."""
    if not token.startswith(_JARO_WINKLER_PREFIX):
        return None
    try:
        threshold = float(token.removeprefix(_JARO_WINKLER_PREFIX))
    except ValueError:
        return None
    # `nan` and `inf` parse and fail this comparison, which is V8's intent: the
    # threshold is a similarity in (0, 1].
    return threshold if 0 < threshold <= 1 else None


def _builder_for(token: str) -> _LevelBuilder | None:
    """The S4.3.1 row for one level token, or ``None`` if the token has no row."""
    if token.startswith(_JARO_WINKLER_PREFIX):
        return LEVEL_TOKENS[_JARO_WINKLER_PREFIX] if _jaro_winkler_threshold(token) else None
    return LEVEL_TOKENS.get(token)


def _unknown_level(column: str, index: int, token: str) -> ConfigError:
    """The S6.1 V8 rejection for one token, so both check sites word it identically."""
    return ConfigError(
        f"{FAILURE_KEYS['V8']}: /comparisons/{column}/levels/{index}: {token!r} is not a "
        f"comparison level of S4.3.1; the vocabulary is {sorted(LEVEL_TOKENS)}, the last "
        f"of which takes a threshold in (0, 1]"
    )


def _reject_unknown_levels(cfg: Config) -> None:
    """S6.1 V8, re-checked here (`comparisons.unknown_level`).

    Run over the whole document before the first Splink object is constructed, so an
    unknown token is a config rejection rather than whatever a level constructor does
    with it. The loader rejects it too, but a caller may hand this function a `Config`
    that never went through `load_config`.
    """
    for column, comparison in cfg.comparisons.items():
        for index, level in enumerate(comparison.levels):
            # YAML's `null` parses to None; a pre-normalization document still has it.
            token = _NULL_TOKEN if level is None else level
            if _builder_for(token) is None:
                raise _unknown_level(column, index, token)


def _comparison(column: str, spec: ComparisonSpec) -> ComparisonCreator:
    """One `comparisons.<column>` entry as a Splink comparison (S4.3.1).

    `NullLevel(column)` first and `ElseLevel()` last are emitted here and nowhere
    else, unconditionally: the config list decides only what goes between them.
    """
    # The union element type is Splink's own: its constructors accept a level either
    # as a creator or as an already-rendered dict, and `list` is invariant.
    levels: list[ComparisonLevelCreator | dict[str, Any]] = [
        LEVEL_TOKENS[_NULL_TOKEN](column, _NULL_TOKEN, spec.tf)
    ]
    for index, level in enumerate(spec.levels):
        token = _NULL_TOKEN if level is None else level
        # A `null` still in the list is redundant, not an error: S6.1 normalization
        # removes it, and skipping it here is what makes a normalized and an
        # un-normalized document render the same settings.
        if token == _NULL_TOKEN:
            continue
        builder = _builder_for(token)
        if builder is None:
            raise _unknown_level(column, index, token)
        levels.append(builder(column, token, spec.tf))
    levels.append(ElseLevel())
    return CustomComparison(
        output_column_name=column,
        comparison_levels=levels,
        # Fixed by the column alone. Deriving it from the token list would make the
        # settings of two documents differing only in a redundant `null` differ too.
        comparison_description=f"{column} comparison",
    )


def build_settings(cfg: Config) -> SettingsCreator:
    """The Splink settings for one config (S4.3, S4.3.1).

    Args:
        cfg: the validated S6 document, normalized or not. The two render
            identically: the `null` token S6.1 strips is skipped here, and nothing
            else the builder reads is normalized.

    Returns:
        A `SettingsCreator` with `record_key` as `unique_id_column_name`, the S4.2
        blocking rules in config order, and one comparison per `comparisons:` key
        whose levels are `NullLevel`, the configured tokens in configured order,
        then `ElseLevel()`.

    Raises:
        ConfigError: a level token has no S4.3.1 row (`comparisons.unknown_level`),
            or the blocking block is invalid (see
            :func:`blocking_rules_from_config`). Both are S4.0 exit `2`.
    """
    _reject_unknown_levels(cfg)
    # The same generator the dbt side consumes, not a second `block_on` call site:
    # the mirror T-BLK-1 checks is only true while there is one.
    _, generated = blocking_rules_from_config(cfg)
    rules: list[BlockingRuleCreator | dict[str, Any]] = list(generated)
    return SettingsCreator(
        link_type=LINK_TYPE,
        comparisons=[_comparison(column, spec) for column, spec in cfg.comparisons.items()],
        blocking_rules_to_generate_predictions=rules,
        unique_id_column_name=UNIQUE_ID_COLUMN,
    )


def settings_json(cfg: Config) -> str:
    """:func:`build_settings` rendered for :data:`SQL_DIALECT`, as canonical JSON.

    Keys are sorted so the text is a function of the config alone: it is compared
    across processes and, from ER-054, persisted as the model artifact, and mapping
    order is not part of what the settings mean.
    """
    rendered = build_settings(cfg).create_settings_dict(SQL_DIALECT)
    return json.dumps(rendered, sort_keys=True, indent=2)
