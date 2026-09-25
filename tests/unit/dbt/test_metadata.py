"""Client fields survive verbatim, including names with JSON-path punctuation."""

import json

import pytest


@pytest.mark.parametrize("source", ["crm", "billing", "webforms"])
def test_metadata_excludes_consumed_fields_and_preserves_extra_values(harness, test_config, source):
    spec = test_config.sources[source]
    payload = {column: "mapped" for column in spec.columns.values()}
    payload.update({spec.record_id_column: "123", spec.updated_at_column: "2026-01-01"})
    payload["persona_id"] = "fixture-only"
    extras = {
        "tier": " Gold ",
        "unknown": "NULL",
        "blank": "",
        "absent": None,
        "x/y~z": {"nested": [1, True, None]},
        "a.b\"'": "雪",
        "metadata": {"already": "structured"},
    }
    payload.update(extras)
    result = harness.eval_macro("source_metadata", json.dumps(payload), spec.model_dump())[0]
    assert json.loads(result["value"]) == extras
    reordered = dict(reversed(list(payload.items())))
    assert (
        harness.eval_macro("source_metadata", json.dumps(reordered), spec.model_dump())[0] == result
    )
    assert set(spec.metadata_columns(payload)) == set(extras)


@pytest.mark.parametrize("payload", [None, "null", "{}"])
def test_empty_or_tombstoned_payload_has_empty_metadata(harness, test_config, payload):
    assert harness.eval_macro(
        "source_metadata", payload, test_config.sources["crm"].model_dump()
    ) == [{"value": "{}"}]


def test_unknown_mapping_is_still_metadata_and_source_renames_are_respected(harness, test_config):
    spec = test_config.sources["crm"].model_copy(deep=True)
    spec.columns["given_name"] = "renamed_first"
    spec.columns["loyalty"] = "loyalty_code"
    payload = {"renamed_first": "Ada", "first_name": "old", "loyalty_code": "L1"}
    result = harness.eval_macro("source_metadata", json.dumps(payload), spec.model_dump())[0]
    assert json.loads(result["value"]) == {"first_name": "old", "loyalty_code": "L1"}
    assert spec.metadata_columns(payload) == ("first_name", "loyalty_code")
