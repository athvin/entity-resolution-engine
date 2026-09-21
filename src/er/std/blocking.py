"""Engine-independent blocking specifications shared by dbt and Splink."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Final

from er.config.schema import Config
from er.config.validators import FAILURE_KEYS, _expression_columns
from er.errors import ConfigError
from er.lake.columns import STD_RECORD_COLUMNS

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


def blocking_payload(cfg: Config) -> BlockingPayload:
    """Render dbt blocking vars without importing the matching engine."""
    return [spec.as_payload() for spec in _key_specs(cfg)]
