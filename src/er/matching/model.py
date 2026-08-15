"""The S4.2 blocking generator: config in, both consumers out (DesignDoc.md S4.2).

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

Nothing here opens a connection or imports `duckdb`. The function is pure — config
in, payload plus rule objects out — so it is callable from a bare unit test and from
the CLI before the lake is attached (S4.0), and `int_blocking_keys` is never read: it
is not an input to scoring (S4.3.4), and this module is the only thing that turns the
config into blocking.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Final

from splink import block_on
from splink.internals.blocking_rule_creator import BlockingRuleCreator

from er.config.schema import Config
from er.config.validators import FAILURE_KEYS, _expression_columns
from er.errors import ConfigError
from er.lake.columns import STD_RECORD_COLUMNS

__all__ = [
    "BLOCKING_DBT_VAR",
    "NULL_EMPTY_PREDICATE",
    "BlockingKeySpec",
    "BlockingPayload",
    "blocking_rules_from_config",
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
