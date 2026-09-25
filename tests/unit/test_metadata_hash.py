"""Metadata-only changes are new deliveries; native and streaming hashes agree."""

import duckdb
import pytest

from er.ingest.hashing import content_hash, content_hash_sql


@pytest.mark.parametrize("value", [None, "", "NULL", " Gold ", "e\u0301", "é", "雪\n\x1f"])
def test_metadata_hash_sql_parity_and_verbatim_values(value):
    row = {"name": "Ada", "a/b~\"'": value, "z": "tail"}
    extra = ["z", "a/b~\"'"]
    expression = content_hash_sql(["name"], list(row), metadata_columns=extra)
    fields = ", ".join('CAST(? AS VARCHAR) AS "' + name.replace('"', '""') + '"' for name in row)
    with duckdb.connect() as connection:
        actual = connection.execute(
            f"SELECT {expression} FROM (SELECT {fields})", list(row.values())
        )
        assert actual.fetchone() == (content_hash(row, ["name"], metadata_columns=extra),)


def test_metadata_changes_names_presence_and_nulls_change_digest():
    rows = [
        {"name": "Ada"},
        {"name": "Ada", "tier": None},
        {"name": "Ada", "tier": ""},
        {"name": "Ada", "tier": "gold"},
        {"name": "Ada", "tier": "Gold"},
        {"name": "Ada", "tier": "gold "},
        {"name": "Ada", "other": "gold"},
    ]
    digests = {
        content_hash(row, ["name"], metadata_columns=[key for key in row if key != "name"])
        for row in rows
    }
    assert len(digests) == len(rows)
    row = {"name": "Ada", "a": "one", "b": "two"}
    assert content_hash(row, ["name"], metadata_columns=["a", "b"]) == content_hash(
        dict(reversed(list(row.items()))), ["name"], metadata_columns=["b", "a"]
    )
