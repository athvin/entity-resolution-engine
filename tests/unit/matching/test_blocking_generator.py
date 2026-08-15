"""S4.2's blocking generator: one config, two consumers, one `expr` string.

Every test here reads either the shipped ``configs/test.yaml`` — the document S6
lists verbatim and CI consumes unmodified — or a hand-built mutation of it. The two
rejection tests assert the *literal* S6.1 failure message key rather than merely that
the call failed, for the reason ``tests/unit/test_config_validators.py`` states: a
document rejected by the wrong rule would pass the weaker assertion.

The dbt half of the mirror (the macro-generated ``int_blocking_keys``) is ER-047's
and the Splink parity check T-BLK-1 is ER-057's; what is checkable without a database
is that both consumers are built from the same strings, in the same order, under the
same NULL/empty predicate — which is what this file checks.
"""

from __future__ import annotations

import ast
import json
from pathlib import Path
from typing import Any

import pytest
import yaml
from splink import block_on

from er.config.loader import load_config
from er.config.schema import Config
from er.config.validators import FAILURE_KEYS
from er.errors import ConfigError, ExitCode
from er.matching.model import (
    BLOCKING_DBT_VAR,
    BlockingKeySpec,
    blocking_rules_from_config,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

#: The `key_type` values of S6's `blocking:` block, in document order. Spelled out
#: rather than read from the config: the order is load-bearing (Splink attributes a
#: pair to the first rule that produced it), so a test deriving it from the same file
#: it is checking would pass on a reordered document.
EXPECTED_KEY_TYPES = ["email_exact", "phone_exact", "name_postal", "dob_name"]

#: The dialect the pipeline runs on; `block_on` renders its SQL per dialect (S2.1).
DIALECT = "duckdb"


def document() -> dict[str, Any]:
    """A fresh, mutable copy of the shipped test document."""
    loaded = yaml.safe_load(TEST_CONFIG.read_text(encoding="utf-8"))
    assert isinstance(loaded, dict), "configs/test.yaml is not a mapping"
    return loaded


def config_from(doc: dict[str, Any]) -> Config:
    """A `Config` built without the loader's S6.1 pass.

    The rejection tests need a document the loader would refuse, so they cannot go
    through ``load_config``: V6 and V7 would reject it there and the generator's own
    rejection — the thing under test — would never be reached.
    """
    return Config.model_validate(doc)


def rule_sql(rule: Any) -> str:
    """The SQL one `BlockingRuleCreator` generates for the pipeline's dialect."""
    return str(rule.get_blocking_rule(DIALECT).blocking_rule_sql)


def test_payload_matches_config_order_and_exprs() -> None:
    """AC1: four entries, config order, byte-identical exprs, one rule each."""
    cfg = load_config(TEST_CONFIG)
    payload, rules = blocking_rules_from_config(cfg)

    assert [entry["key_type"] for entry in payload] == EXPECTED_KEY_TYPES
    assert [entry["expr"] for entry in payload] == [rule.expr for rule in cfg.blocking]
    assert len(rules) == len(payload) == 4

    # The rule and the payload entry at the same index are the same key: this is the
    # mirror T-BLK-1 later checks against a real Splink run. The comparison is against
    # `block_on` applied to the payload's own `expr`, never against the raw `expr`
    # itself: S4.2 puts the byte-identity at the `block_on` input, and `block_on`
    # re-renders through sqlglot (`substr` becomes `SUBSTRING`) and inserts `l.` / `r.`
    # inside the expression, so containment is false for every multi-column expr.
    for entry, rule in zip(payload, rules, strict=True):
        assert rule_sql(rule) == rule_sql(block_on(entry["expr"]))


def test_payload_is_stable_and_serialisable(tmp_path: Path) -> None:
    """AC2: JSON-round-trippable, and invariant under unrelated key reordering."""
    cfg = load_config(TEST_CONFIG)
    first, _ = blocking_rules_from_config(cfg)
    second, _ = blocking_rules_from_config(cfg)
    encoded = json.dumps(first, sort_keys=True)

    assert json.dumps(second, sort_keys=True) == encoded
    assert json.loads(encoded) == first, "the payload must survive the --vars round trip"

    doc = document()
    # Reverse every mapping the payload does not read, plus the top level itself. A
    # payload that changed here would mean the generator had picked up document
    # order from somewhere other than `blocking:`.
    doc["sources"] = dict(reversed(list(doc["sources"].items())))
    doc["comparisons"] = dict(reversed(list(doc["comparisons"].items())))
    reordered = dict(reversed(list(doc.items())))
    path = tmp_path / "reordered.yaml"
    path.write_text(yaml.safe_dump(reordered, sort_keys=False), encoding="utf-8")

    shuffled, _ = blocking_rules_from_config(load_config(path))
    assert json.dumps(shuffled, sort_keys=True) == encoded


def test_duplicate_key_type_rejected() -> None:
    """AC3: two entries sharing a `key_type` are `blocking.duplicate_key_type`."""
    doc = document()
    doc["blocking"].append({"key_type": "email_exact", "expr": "family_name"})

    with pytest.raises(ConfigError) as raised:
        blocking_rules_from_config(config_from(doc))

    message = str(raised.value)
    assert message.startswith(FAILURE_KEYS["V7"] + ":")
    assert "email_exact" in message, "the rejection must name the repeated key_type"
    assert raised.value.code == ExitCode.CONFIG


def test_unknown_column_rejected() -> None:
    """AC4: an `expr` over a column `int_std_records` does not have is `columns.unknown`."""
    doc = document()
    doc["blocking"].append({"key_type": "nickname_exact", "expr": "nickname"})

    with pytest.raises(ConfigError) as raised:
        blocking_rules_from_config(config_from(doc))

    message = str(raised.value)
    assert message.startswith(FAILURE_KEYS["V6"] + ":")
    assert "nickname" in message, "the rejection must name the offending column"
    assert raised.value.code == ExitCode.CONFIG

    # The accepting half of AC4: the six S5 columns a v1 blocking key may name.
    doc = document()
    doc["blocking"] = [
        {"key_type": "postal", "expr": "addr_postal"},
        {"key_type": "family", "expr": "family_name"},
        {"key_type": "mail", "expr": "email"},
        {"key_type": "phone", "expr": "phone_e164"},
        {"key_type": "dob", "expr": "birth_date"},
        {"key_type": "given", "expr": "given_name"},
    ]

    payload, rules = blocking_rules_from_config(config_from(doc))

    assert len(payload) == len(rules) == 6


def test_null_empty_predicate_is_the_spec_template() -> None:
    """AC5: the rendered predicate is S4.2's template, character for character."""
    cfg = load_config(TEST_CONFIG)
    payload, _ = blocking_rules_from_config(cfg)

    for entry in payload:
        # The S4.2 template, transcribed here rather than imported: a test comparing
        # the module against its own constant would pass on a rewritten template.
        expr = entry["expr"]
        assert entry["where"] == f"{expr} is not null and {expr} <> ''"

    spec = BlockingKeySpec.from_rule("email_exact", "email")
    assert spec.where == "email is not null and email <> ''"


def test_generator_is_pure_no_duckdb_import() -> None:
    """AC6: the module names no `duckdb` symbol and opens no connection."""
    source = (REPO_ROOT / "src" / "er" / "matching" / "model.py").read_text(encoding="utf-8")
    tree = ast.parse(source)

    imported: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imported.extend(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module is not None:
            imported.append(node.module)
    assert not [name for name in imported if name.split(".")[0] == "duckdb"]

    names = {node.id for node in ast.walk(tree) if isinstance(node, ast.Name)}
    assert "duckdb" not in names
    attributes = {node.attr for node in ast.walk(tree) if isinstance(node, ast.Attribute)}
    assert "connect" not in attributes, "the generator must open no database connection"

    # The var name the CLI passes it under is a plain string, so `render_dbt_vars`
    # needs nothing from an engine either.
    assert isinstance(BLOCKING_DBT_VAR, str)


def test_added_rule_appends_only() -> None:
    """AC7: a fifth `blocking:` entry adds exactly one payload entry and one rule."""
    baseline_payload, baseline_rules = blocking_rules_from_config(load_config(TEST_CONFIG))

    doc = document()
    added = {"key_type": "email_postal", "expr": "email || '|' || addr_postal"}
    doc["blocking"].append(added)
    payload, rules = blocking_rules_from_config(config_from(doc))

    assert len(payload) == len(baseline_payload) + 1
    assert len(rules) == len(baseline_rules) + 1
    assert payload[:-1] == baseline_payload, "an appended entry changed an earlier one"
    assert [rule_sql(rule) for rule in rules[:-1]] == [rule_sql(rule) for rule in baseline_rules]
    assert payload[-1]["key_type"] == added["key_type"]
    assert payload[-1]["expr"] == added["expr"]
