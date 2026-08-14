"""The S6 config document is asserted against DesignDoc.md, not against itself.

``configs/test.yaml`` is the file the fixtures and CI consume verbatim, so the test
that matters most compares it to the fenced block in the spec rather than to a copy
kept here. The rest of the file walks S6.1: the single-block validators this ticket
owns (V9-V16) by their literal failure message keys, the normalization that runs
after them, and the S5.2 digest computed over the result.

V1-V8 are deliberately absent — they are cross-field and referential, and
``tests/unit/test_config_validators.py`` owns them. The union of the two files is
the "each has a unit test" requirement of S6.1.
"""

from __future__ import annotations

import copy
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
import yaml
from pydantic import BaseModel

from er.config import schema
from er.config.hashing import config_hash
from er.config.loader import ConfigValidationError, load_config
from er.config.schema import CONFIG_BLOCKS, Config

REPO_ROOT = Path(__file__).resolve().parents[2]
DESIGN_DOC = REPO_ROOT / "DesignDoc.md"
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"
DEFAULT_CONFIG = REPO_ROOT / "configs" / "default.yaml"

# AC1's list, written out rather than derived, so a block added to the model with
# no S6 row fails here instead of being blessed by the code that added it.
EXPECTED_BLOCKS = {
    "tenant",
    "thresholds",
    "standardization",
    "sources",
    "blocking",
    "comparisons",
    "survivorship",
    "training",
    "storage",
    "versions",
    "generator",
    "clustering",
    "coherence",
    "correction_pass",
}


def spec_document() -> dict[str, Any]:
    """The fenced YAML block under the S6 anchor of DesignDoc.md, parsed."""
    text = DESIGN_DOC.read_text(encoding="utf-8")
    anchor = text.find('<a id="s6"></a>')
    assert anchor != -1, "DesignDoc.md has no S6 anchor"
    fence = text.find("```yaml", anchor)
    assert fence != -1, "S6 has no fenced yaml block"
    body_start = fence + len("```yaml")
    body_end = text.find("```", body_start)
    assert body_end != -1, "the S6 yaml block is unterminated"
    # The block must be the config listing itself, not one from a later section.
    assert body_end < text.find('<a id="s6-1"></a>'), "the fenced block found is not S6's listing"
    document = yaml.safe_load(text[body_start:body_end])
    assert isinstance(document, dict)
    return document


def write_config(tmp_path: Path, document: dict[str, Any], name: str = "config.yaml") -> Path:
    path = tmp_path / name
    path.write_text(yaml.safe_dump(document, sort_keys=False), encoding="utf-8")
    return path


def reject(tmp_path: Path, document: dict[str, Any]) -> ConfigValidationError:
    """Load a document that must fail validation and return the raised error."""
    with pytest.raises(ConfigValidationError) as raised:
        load_config(write_config(tmp_path, document))
    return raised.value


def model_classes() -> list[type[BaseModel]]:
    return [
        obj
        for obj in vars(schema).values()
        if isinstance(obj, type) and issubclass(obj, BaseModel) and obj is not BaseModel
    ]


def test_fourteen_blocks_exact_and_extra_forbidden(tmp_path: Path) -> None:
    assert set(Config.model_fields) == EXPECTED_BLOCKS
    assert set(CONFIG_BLOCKS) == EXPECTED_BLOCKS
    assert len(CONFIG_BLOCKS) == len(EXPECTED_BLOCKS), "CONFIG_BLOCKS repeats a block"

    document = spec_document()
    document["tennant"] = "typo"
    error = reject(tmp_path, document)
    assert error.errors[0].loc == ("tennant",)

    # AC7's static half: `mypy --strict` cannot see an `Any` that is inside an
    # annotation the model still type-checks against, so the surface is asserted.
    for model in model_classes():
        for name, field in model.model_fields.items():
            assert "Any" not in str(field.annotation), f"{model.__name__}.{name} is annotated Any"


def test_test_yaml_matches_spec_s6_block() -> None:
    committed = yaml.safe_load(TEST_CONFIG.read_text(encoding="utf-8"))
    assert committed == spec_document()

    config = load_config(TEST_CONFIG)
    assert config.tenant == "test"
    # The reference document is not the fixture, but it must still load.
    assert load_config(DEFAULT_CONFIG).tenant == "default"


def test_u_seed_is_required_with_error_path(tmp_path: Path) -> None:
    document = spec_document()
    del document["training"]["u_seed"]

    error = reject(tmp_path, document)
    assert error.errors[0].loc == ("training", "u_seed")
    assert error.errors[0].key == "training.u_seed.required"
    assert error.pointer == "/training/u_seed"

    document["training"]["u_seed"] = 20260101
    assert load_config(write_config(tmp_path, document)).training.u_seed == 20260101


@pytest.mark.parametrize(
    ("validator", "mutate", "key"),
    [
        (
            "V9",
            lambda d: d["training"].update(em_blocking_rules=["l.email = r.email"]),
            "training.em_blocking_rules.min_items",
        ),
        (
            "V11",
            lambda d: d["sources"]["crm"]["columns"].pop("email"),
            "sources.columns.incomplete",
        ),
        ("V12", lambda d: d["clustering"].update(max_iterations=0), "clustering.bounds"),
        ("V13", lambda d: d["versions"].update(std_version=""), "versions.required"),
        ("V14", lambda d: d["storage"].update(data_path="s3://lake/er"), "storage.uri"),
        (
            "V15",
            lambda d: d["correction_pass"].update(cadence="0 3 * *"),
            "correction_pass.cadence",
        ),
        (
            "V16",
            lambda d: d["standardization"].update(phone_default_region="USA"),
            "standardization.invalid",
        ),
    ],
)
def test_single_block_validator_message_keys(
    tmp_path: Path,
    validator: str,
    mutate: Callable[[dict[str, Any]], object],
    key: str,
) -> None:
    document = spec_document()
    mutate(document)
    error = reject(tmp_path, document)
    assert [e.key for e in error.errors] == [key], f"{validator} raised the wrong S6.1 key"
    assert error.code == 2


def test_normalization_drops_null_token_and_appends_record_key() -> None:
    raw = spec_document()
    assert any(None in spec["levels"] for spec in raw["comparisons"].values()), (
        "the S6 listing no longer carries a null token, so this test proves nothing"
    )

    config = load_config(TEST_CONFIG)
    for column, comparison in config.comparisons.items():
        assert None not in comparison.levels, f"comparisons.{column} kept the null token"
        assert comparison.levels == [
            level for level in raw["comparisons"][column]["levels"] if level is not None
        ]
    for attribute, chain in config.survivorship.items():
        assert chain[-1] == "record_key ASC", f"survivorship.{attribute} has no terminal rule"
        assert chain[:-1] == raw["survivorship"][attribute]


def test_config_hash_is_stable_under_reserialisation(tmp_path: Path) -> None:
    baseline = config_hash(load_config(TEST_CONFIG))
    document = spec_document()

    # Mapping keys reordered, comments added, flow mappings restyled as blocks and
    # long scalars folded: everything a human edit can change without changing the
    # document's meaning.
    restyled = tmp_path / "restyled.yaml"
    restyled.write_text(
        "# a comment the digest must not see\n"
        + yaml.safe_dump(document, sort_keys=True, default_flow_style=False, width=40)
        + "\n# and a trailing one\n",
        encoding="utf-8",
    )
    assert config_hash(load_config(restyled)) == baseline

    # S6.1: two configs differing only in a redundant null token hash identically.
    document["comparisons"]["addr_postal"]["levels"].append(None)
    assert config_hash(load_config(write_config(tmp_path, document))) == baseline


def test_config_hash_changes_on_value_change(tmp_path: Path) -> None:
    baseline = config_hash(load_config(TEST_CONFIG))
    document = spec_document()
    document["thresholds"]["auto_merge"] = 0.94
    assert config_hash(load_config(write_config(tmp_path, document))) != baseline

    # Same document, loaded twice from different files: the digest is a function of
    # the content, not of the path or of dict iteration order.
    twin = copy.deepcopy(spec_document())
    assert config_hash(load_config(write_config(tmp_path, twin, "twin.yaml"))) == baseline


def test_loader_raises_with_code_2(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    assert ConfigValidationError.code == 2

    missing = tmp_path / "absent.yaml"
    with pytest.raises(ConfigValidationError) as unreadable:
        load_config(missing)
    assert unreadable.value.code == 2
    assert unreadable.value.pointer == "/"

    not_a_mapping = tmp_path / "list.yaml"
    not_a_mapping.write_text("- one\n- two\n", encoding="utf-8")
    with pytest.raises(ConfigValidationError):
        load_config(not_a_mapping)

    unparsable = tmp_path / "broken.yaml"
    unparsable.write_text("tenant: [unclosed\n", encoding="utf-8")
    with pytest.raises(ConfigValidationError):
        load_config(unparsable)

    # $ER_CONFIG is the only environment variable this package reads (S6.1).
    monkeypatch.delenv("ER_CONFIG", raising=False)
    with pytest.raises(ConfigValidationError):
        load_config()
    monkeypatch.setenv("ER_CONFIG", str(TEST_CONFIG))
    assert load_config().tenant == "test"
