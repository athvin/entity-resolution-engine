"""S6.1 V1-V8: the cross-field and referential validators, one failing document each.

Each test mutates the shipped ``configs/test.yaml`` — which ``test_config_schema.py``
holds equal to the S6 listing verbatim — into a document that violates exactly one
rule, and asserts the *literal* failure message key of the S6.1 table. Asserting only
that validation failed would pass for a document rejected by the wrong rule, which is
the failure mode S8.4 names.

V9-V16 are ``test_config_schema.py``'s; the union of the two files is S6.1's "each has
a unit test".
"""

from __future__ import annotations

import subprocess
import sys
import textwrap
from pathlib import Path
from typing import Any

import pytest
import yaml

from er.config.loader import ConfigValidationError, load_config
from er.config.schema import Config
from er.config.validators import (
    COMPARISON_LEVEL_TOKENS,
    FAILURE_KEYS,
    SURVIVORSHIP_RULES,
    expand_survivorship_keys,
    validate_cross_fields,
)
from er.lake.columns import GOLDEN_SURVIVABLE_COLUMNS

REPO_ROOT = Path(__file__).resolve().parents[2]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"
DEFAULT_CONFIG = REPO_ROOT / "configs" / "default.yaml"


def document() -> dict[str, Any]:
    """A fresh, mutable copy of the shipped test document."""
    loaded = yaml.safe_load(TEST_CONFIG.read_text(encoding="utf-8"))
    assert isinstance(loaded, dict), "configs/test.yaml is not a mapping"
    return loaded


def write(tmp_path: Path, doc: dict[str, Any], name: str = "config.yaml") -> Path:
    path = tmp_path / name
    path.write_text(yaml.safe_dump(doc, sort_keys=False), encoding="utf-8")
    return path


def rejected_key(tmp_path: Path, doc: dict[str, Any]) -> str:
    """The single S6.1 failure key a document is rejected with, plus S4.0's exit code."""
    with pytest.raises(ConfigValidationError) as raised:
        load_config(write(tmp_path, doc))
    error = raised.value
    assert error.code == 2, "S4.0 requires exit 2 for a config failure"
    assert len(error.errors) == 1, f"expected one rejection, got {[e.key for e in error.errors]}"
    return error.errors[0].key


def accept(tmp_path: Path, doc: dict[str, Any]) -> Config:
    return load_config(write(tmp_path, doc))


def test_v1_threshold_ordering_key(tmp_path: Path) -> None:
    doc = document()
    doc["thresholds"]["review_low"] = doc["thresholds"]["auto_merge"]
    assert rejected_key(tmp_path, doc) == FAILURE_KEYS["V1"] == "thresholds.ordering"

    doc = document()
    doc["thresholds"]["review_low"] = 0.0
    assert rejected_key(tmp_path, doc) == "thresholds.ordering"

    doc = document()
    doc["thresholds"].update(review_low=0.6, auto_merge=1.5)
    assert rejected_key(tmp_path, doc) == "thresholds.ordering"

    # The closed upper end is legal: p = 1.0 is a probability, and the gray band
    # [0.6, 1.0) is non-empty (S4.3).
    doc = document()
    doc["thresholds"].update(review_low=0.6, auto_merge=1.0)
    assert accept(tmp_path, doc).thresholds.auto_merge == 1.0


def test_v2_survivorship_keyset_is_bidirectional(tmp_path: Path) -> None:
    # Direction one: a survivable golden column with no rule.
    doc = document()
    del doc["survivorship"]["address"]
    assert rejected_key(tmp_path, doc) == FAILURE_KEYS["V2"] == "survivorship.keyset"

    # Direction two: a rule with no column. `email_valid` is the tempting one, and
    # S5.0 deliberately excludes it -- it is an input to `validated`, not survivable.
    doc = document()
    doc["survivorship"]["email_valid"] = ["recency"]
    assert rejected_key(tmp_path, doc) == "survivorship.keyset"
    assert "email_valid" not in GOLDEN_SURVIVABLE_COLUMNS
    assert "phone_valid" not in GOLDEN_SURVIVABLE_COLUMNS

    # Spelling the six address columns out instead of `address` expands to the same
    # set but is still rejected: S4.6's lineage vocabulary names `address`, and its
    # six columns must come from one winning record.
    doc = document()
    del doc["survivorship"]["address"]
    for column in GOLDEN_SURVIVABLE_COLUMNS:
        if column.startswith("addr_"):
            doc["survivorship"][column] = ["recency", "source_priority"]
    assert rejected_key(tmp_path, doc) == "survivorship.keyset"

    accepted = load_config(TEST_CONFIG).survivorship
    assert expand_survivorship_keys(accepted) == frozenset(GOLDEN_SURVIVABLE_COLUMNS)


def test_v3_unknown_survivorship_rule_key(tmp_path: Path) -> None:
    for rule in ("phonetic", "priority", "record_key ASC"):
        doc = document()
        doc["survivorship"]["given_name"] = [rule, "recency"]
        assert rejected_key(tmp_path, doc) == FAILURE_KEYS["V3"] == "survivorship.unknown_rule"

    # The vocabulary is S4.6's dispatch table and nothing else.
    assert set(SURVIVORSHIP_RULES) == {
        "source_priority",
        "recency",
        "frequency",
        "completeness",
        "validated",
    }
    # All five at once, on the one attribute whose `validated` rule has an input (V4).
    doc = document()
    doc["survivorship"]["email"] = list(SURVIVORSHIP_RULES)
    assert accept(tmp_path, doc).survivorship["email"][:-1] == list(SURVIVORSHIP_RULES)


def test_v4_validated_requires_valid_column(tmp_path: Path) -> None:
    for attribute in ("given_name", "family_name", "birth_date", "address"):
        doc = document()
        doc["survivorship"][attribute] = ["validated", "recency"]
        assert (
            rejected_key(tmp_path, doc)
            == FAILURE_KEYS["V4"]
            == "survivorship.validated_missing_column"
        )

    # The two attributes that do have an input: `int_std_records` carries
    # `email_valid` and `phone_valid` (S4.6).
    config = load_config(TEST_CONFIG)
    assert "validated" in config.survivorship["email"]
    assert "validated" in config.survivorship["phone_e164"]


def test_v5_chain_must_separate_same_source_records(tmp_path: Path) -> None:
    doc = document()
    doc["survivorship"]["given_name"] = ["source_priority"]
    assert rejected_key(tmp_path, doc) == FAILURE_KEYS["V5"] == "survivorship.not_separating"

    # An empty chain is the same defect stated more briefly: the terminal
    # `record_key ASC` would be the whole chain.
    doc = document()
    doc["survivorship"]["given_name"] = []
    assert rejected_key(tmp_path, doc) == "survivorship.not_separating"

    doc = document()
    doc["survivorship"]["given_name"] = ["source_priority", "recency"]
    assert accept(tmp_path, doc).survivorship["given_name"][-1] == "record_key ASC"


def test_v6_unknown_column_in_blocking_and_comparisons(tmp_path: Path) -> None:
    doc = document()
    doc["blocking"][0] = {"key_type": "nickname_exact", "expr": "nickname"}
    assert rejected_key(tmp_path, doc) == FAILURE_KEYS["V6"] == "columns.unknown"

    doc = document()
    doc["blocking"][2]["expr"] = "substr(family_name,1,4) || '|' || zip_code"
    assert rejected_key(tmp_path, doc) == "columns.unknown"

    doc = document()
    doc["comparisons"]["nickname"] = {"levels": ["exact", None]}
    assert rejected_key(tmp_path, doc) == "columns.unknown"

    # A function name is not a column reference, and neither is the text inside a
    # string literal: the shipped `name_postal` rule uses both.
    doc = document()
    doc["blocking"][2]["expr"] = "substr(family_name,1,4) || 'nickname' || addr_postal"
    assert accept(tmp_path, doc).blocking[2].key_type == "name_postal"


def test_v7_duplicate_key_type(tmp_path: Path) -> None:
    doc = document()
    doc["blocking"].append({"key_type": "email_exact", "expr": "email_valid"})
    assert rejected_key(tmp_path, doc) == FAILURE_KEYS["V7"] == "blocking.duplicate_key_type"

    # Two rules building different keys from the same column are fine; only the
    # key_type has to be unique.
    doc = document()
    doc["blocking"].append({"key_type": "email_prefix", "expr": "substr(email,1,4)"})
    assert len(accept(tmp_path, doc).blocking) == 5


def test_v8_unknown_level_token_rejects_phonetic(tmp_path: Path) -> None:
    rejected = ("phonetic", "jaro_winkler:1.5", "jaro_winkler:0", "jaro_winkler:", "exact_match")
    for token in rejected:
        doc = document()
        doc["comparisons"]["given_name"]["levels"] = ["exact", token, None]
        assert rejected_key(tmp_path, doc) == FAILURE_KEYS["V8"] == "comparisons.unknown_level"

    assert "phonetic" not in COMPARISON_LEVEL_TOKENS, "S4.3.1 deletes the phonetic level"
    assert COMPARISON_LEVEL_TOKENS == {
        "exact",
        "null",
        "username_exact",
        "variant_match",
        "dob_same_year_month",
    }

    # The closed upper end of the threshold and the `null` token S6.1 normalization
    # removes afterwards are both accepted here.
    doc = document()
    doc["comparisons"]["given_name"]["levels"] = ["exact", "jaro_winkler:1.0", None]
    assert accept(tmp_path, doc).comparisons["given_name"].levels == ["exact", "jaro_winkler:1.0"]


def test_shipped_configs_pass_all_validators() -> None:
    for path in (TEST_CONFIG, DEFAULT_CONFIG):
        raw = yaml.safe_load(path.read_text(encoding="utf-8"))
        # V9-V16 on the way in, then V1-V8 over the pre-normalization document: the
        # order `load_config` uses, asserted here without normalization in between.
        validate_cross_fields(Config.model_validate(raw))
        config = load_config(path)
        assert expand_survivorship_keys(config.survivorship) == frozenset(GOLDEN_SURVIVABLE_COLUMNS)


def test_validation_opens_no_lake_connection(tmp_path: Path) -> None:
    doc = document()
    doc["thresholds"]["review_low"] = doc["thresholds"]["auto_merge"]
    invalid = write(tmp_path, doc, "invalid.yaml")

    # A fresh interpreter, because this test's own process may have imported an engine
    # for another reason. S4.0 requires the document to be rejected before a lake
    # connection exists, which is a property of the import graph rather than of the
    # call order, so the assertion is about `sys.modules`.
    probe = textwrap.dedent(
        """
        import sys

        from er.config.loader import ConfigValidationError, load_config

        try:
            load_config(sys.argv[1])
        except ConfigValidationError as error:
            print(error.errors[0].key)
        else:
            raise SystemExit("the document should have been rejected")

        for engine in ("duckdb", "splink", "psycopg", "boto3"):
            if engine in sys.modules:
                raise SystemExit(f"config validation imported {engine}")
        """
    )
    result = subprocess.run(
        [sys.executable, "-c", probe, str(invalid)],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert result.stdout.strip() == "thresholds.ordering"
