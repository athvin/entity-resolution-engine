"""S4.3.1's settings builder: the token table, and the two levels nothing configures.

Every test reads the rendered settings *dict* — ``SettingsCreator.create_settings_dict``
for the pipeline's dialect — rather than the creator objects, because the dict is what
Splink actually executes and what S4.3.2 persists as the model artifact. A test over
the creators would pass on a construct that renders to the wrong SQL.

Nothing here opens a connection or builds a `Linker`: the settings are a pure function
of the config (ER-049 owns the linker), so the whole file runs without Docker.

The `null`-token and else-level tests are the two that exist because their subject is
invisible in the config: `NullLevel` first and `ElseLevel()` last are emitted whatever
`levels` holds, and a missing else level shows up only as a NULL match weight on the
pairs that matched nothing.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
import yaml

from er.config.loader import load_config
from er.config.schema import Config, normalize
from er.config.validators import FAILURE_KEYS
from er.errors import ConfigError, ExitCode
from er.matching.model import (
    DOB_SAME_YEAR_MONTH_SQL,
    LEVEL_TOKENS,
    SQL_DIALECT,
    USERNAME_EXACT_SQL,
    blocking_rules_from_config,
    build_settings,
    settings_json,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"
MODEL_SOURCE = REPO_ROOT / "src" / "er" / "matching" / "model.py"

#: The `comparisons:` keys of S6, in document order. Spelled out rather than read from
#: the config for the reason the blocking tests spell out their key types: a test that
#: derives its expectation from the file under test asserts nothing about that file.
EXPECTED_COMPARISONS = [
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "birth_date",
    "addr_postal",
]

#: The three columns S6 marks `tf: true`. TF reaches their exact-match level and,
#: per S4.3.1, no other level of any comparison.
TF_COLUMNS = {"given_name", "family_name", "email"}

#: Splink's rendered marker for `ElseLevel()`. It is a literal, not a SQL predicate.
ELSE_CONDITION = "ELSE"


def document() -> dict[str, Any]:
    """A fresh, mutable copy of the shipped test document."""
    loaded = yaml.safe_load(TEST_CONFIG.read_text(encoding="utf-8"))
    assert isinstance(loaded, dict), "configs/test.yaml is not a mapping"
    return loaded


def config_from(doc: dict[str, Any]) -> Config:
    """A `Config` built without the loader's S6.1 pass.

    The rejection test needs a document the loader would refuse, so it cannot go
    through ``load_config``: V8 would reject it there and the builder's own
    rejection — the thing under test — would never be reached.
    """
    return Config.model_validate(doc)


def settings_dict(cfg: Config) -> dict[str, Any]:
    """The settings as Splink renders them for the pipeline's dialect."""
    return build_settings(cfg).create_settings_dict(SQL_DIALECT)


def levels_of(settings: dict[str, Any], column: str) -> list[dict[str, Any]]:
    """The rendered comparison levels of one comparison, in emission order."""
    for comparison in settings["comparisons"]:
        if comparison["output_column_name"] == column:
            levels: list[dict[str, Any]] = comparison["comparison_levels"]
            return levels
    raise AssertionError(f"no comparison for {column!r}")


def conditions_of(settings: dict[str, Any], column: str) -> list[str]:
    """The `sql_condition` of every level of one comparison, in emission order."""
    return [str(level["sql_condition"]) for level in levels_of(settings, column)]


def rule_sql(rule: Any) -> str:
    """The SQL one `BlockingRuleCreator` generates for the pipeline's dialect."""
    return str(rule.get_blocking_rule(SQL_DIALECT).blocking_rule_sql)


def test_unique_id_and_blocking_rules_come_from_the_generator() -> None:
    """AC1: `record_key`, `dedupe_only`, and S4.2's rules in S4.2's order."""
    cfg = load_config(TEST_CONFIG)
    settings = settings_dict(cfg)

    assert settings["unique_id_column_name"] == "record_key"
    assert settings["link_type"] == "dedupe_only"

    # Element-wise by generated SQL, not by object identity: what has to agree with
    # the dbt mirror is the SQL, and the settings builder must not have re-derived
    # the rules from a second `block_on` call site.
    _, rules = blocking_rules_from_config(cfg)
    rendered = [
        str(rule["blocking_rule"]) for rule in settings["blocking_rules_to_generate_predictions"]
    ]
    assert rendered == [rule_sql(rule) for rule in rules]

    assert [c["output_column_name"] for c in settings["comparisons"]] == EXPECTED_COMPARISONS


def test_null_level_first_and_else_level_last_on_every_comparison() -> None:
    """AC2: both bracketing levels on all six comparisons, whatever `levels` holds.

    Deleting the `ElseLevel()` emission fails the last assertion; deleting the
    `NullLevel` emission fails the first. Neither is driven by the config: S6.1
    strips the `null` token and no document names an else level at all.
    """
    cfg = load_config(TEST_CONFIG)
    settings = settings_dict(cfg)

    assert len(settings["comparisons"]) == len(EXPECTED_COMPARISONS)
    for column in EXPECTED_COMPARISONS:
        levels = levels_of(settings, column)
        first, last = levels[0], levels[-1]

        assert first["is_null_level"] is True, f"{column}: first level is not the null level"
        # On its OWN column: a null test on some other column would still be a null
        # level, and would make the whole comparison unreachable for the wrong pairs.
        assert str(first["sql_condition"]) == f'"{column}_l" IS NULL OR "{column}_r" IS NULL'
        assert str(last["sql_condition"]) == ELSE_CONDITION, f"{column}: no else level"
        assert ELSE_CONDITION not in conditions_of(settings, column)[:-1]


def test_level_token_sql_is_exact() -> None:
    """AC3: the four tokens whose SQL S4.3.1 pins, rendered character for character."""
    cfg = load_config(TEST_CONFIG)
    settings = settings_dict(cfg)

    assert USERNAME_EXACT_SQL == "split_part(email_l,'@',1) = split_part(email_r,'@',1)"
    assert DOB_SAME_YEAR_MONTH_SQL == (
        "date_trunc('month', birth_date_l) = date_trunc('month', birth_date_r)"
    )
    assert USERNAME_EXACT_SQL in conditions_of(settings, "email")
    assert DOB_SAME_YEAR_MONTH_SQL in conditions_of(settings, "birth_date")

    # `variant_match` intersects `name_variants` whatever attribute it is configured
    # under, and fires on one shared element (S4.3.1).
    variant = conditions_of(settings, "given_name")[3]
    assert variant == 'array_length(list_intersect("name_variants_l", "name_variants_r")) >= 1'

    # `jaro_winkler:0.90` carries its threshold in the token; 0.90 must reach the SQL
    # as 0.9 and not as a default.
    jaro = conditions_of(settings, "given_name")[2]
    assert jaro == 'jaro_winkler_similarity("given_name_l", "given_name_r") >= 0.9'

    # The table has exactly the six S4.3.1 rows.
    assert set(LEVEL_TOKENS) == {
        "exact",
        "jaro_winkler:",
        "null",
        "username_exact",
        "variant_match",
        "dob_same_year_month",
    }


def test_tf_flag_only_on_exact_levels_of_tf_columns() -> None:
    """AC4: TF on the exact level of the three `tf: true` columns and nowhere else."""
    cfg = load_config(TEST_CONFIG)
    settings = settings_dict(cfg)

    adjusted: set[tuple[str, str]] = set()
    for comparison in settings["comparisons"]:
        column = str(comparison["output_column_name"])
        for level in comparison["comparison_levels"]:
            tf_column = level.get("tf_adjustment_column")
            if tf_column is None:
                continue
            # TF on a non-exact level would weight a fuzzy agreement by the frequency
            # of a value neither side necessarily has (S4.3.1).
            assert str(level["sql_condition"]) == f'"{column}_l" = "{column}_r"', (
                f"{column}: term frequency adjustment on a level that is not an exact match"
            )
            adjusted.add((column, str(tf_column)))

    assert adjusted == {(column, column) for column in TF_COLUMNS}


def test_null_token_is_normalisation_invariant() -> None:
    """AC5: a redundant `null` in `levels` changes nothing in the rendered settings."""
    cfg = load_config(TEST_CONFIG)
    with_null = config_from(document())

    # The shipped document lists `null` on every comparison, so `normalize` is the
    # mutation under test: the two documents differ exactly by that token.
    assert any(None in comparison.levels for comparison in with_null.comparisons.values())
    without_null = normalize(with_null)
    assert not any(None in comparison.levels for comparison in without_null.comparisons.values())

    rendered = json.dumps(settings_dict(with_null), sort_keys=True)
    assert json.dumps(settings_dict(without_null), sort_keys=True) == rendered
    assert json.dumps(settings_dict(cfg), sort_keys=True) == rendered


def test_unknown_level_token_rejected() -> None:
    """AC6: an out-of-vocabulary token is `comparisons.unknown_level`, exit 2."""
    doc = document()
    doc["comparisons"]["family_name"]["levels"] = ["exact", "soundex", None]

    with pytest.raises(ConfigError) as raised:
        build_settings(config_from(doc))

    message = str(raised.value)
    assert message.startswith(FAILURE_KEYS["V8"] + ":")
    assert "soundex" in message, "the rejection must name the offending token"
    assert "family_name" in message, "the rejection must locate the offending comparison"
    assert raised.value.code == ExitCode.CONFIG

    # A malformed threshold is the same rejection: the token is only a level of
    # S4.3.1 while its threshold is a similarity in (0, 1].
    doc = document()
    doc["comparisons"]["given_name"]["levels"] = ["exact", "jaro_winkler:1.5", None]
    with pytest.raises(ConfigError) as raised:
        build_settings(config_from(doc))
    assert str(raised.value).startswith(FAILURE_KEYS["V8"] + ":")


def test_no_birth_date_precision_and_no_phonetic() -> None:
    """AC7: S4.2 deleted the precision column and S4.3.1 deleted the sound-alike level."""
    cfg = load_config(TEST_CONFIG)

    # Not merely "no comparison on it": the string must not reach the settings at all,
    # since `parse_date` never persists a precision for anything to read (S4.2).
    assert "birth_date_precision" not in settings_json(cfg)

    source = MODEL_SOURCE.read_text(encoding="utf-8")
    assert "phonetic" not in source, "S4.3.1 has no such level; it is deleted from the spec"
    assert "birth_date_precision" not in source


def test_settings_json_is_deterministic() -> None:
    """AC8: the same config renders the same bytes, with no database in the path."""
    cfg = load_config(TEST_CONFIG)

    first = settings_json(cfg)
    assert settings_json(cfg) == first
    # Round-trippable, because S4.3.2 writes this text to the model URI and reads it
    # back as the frozen model.
    assert json.loads(first) == settings_dict(cfg)

    # The builder is pure config->settings: a linker, and therefore a connection, is
    # ER-049's. `duckdb` never being imported is what makes that checkable here.
    source = MODEL_SOURCE.read_text(encoding="utf-8")
    assert "import duckdb" not in source
    assert "DuckDBAPI" not in source
