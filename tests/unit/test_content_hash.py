"""Unit tests for the normative ``content_hash`` of DesignDoc.md S4.1.

The committed vector file is the point of this suite. Every vector carries the
preimage it hashes, written out as hex, so each digest is recomputed here with
``hashlib.sha256(bytes.fromhex(preimage_hex))`` — an oracle that never imports the
module under test. A vector file generated *from* the module would agree with any
future rewrite of it, including a wrong one; this one disagrees, which is what makes
T-IDEM-1a and T-IDEM-1 reproducible.

The remaining tests name the S4.1 properties rather than the implementation: NFC
equivalence, the deliberate NULL/empty-string collision, declared column order,
exclusion of ``ingested_at``/``ingest_batch_id``/``std_version``, the unreachability
of the S4.1.1 tombstone sentinel, the separator ambiguity S4.1's encoding accepts,
and the stdlib-only import surface that lets the fixture and generator layers import
this digest without dragging in config or lake code.
"""

from __future__ import annotations

import ast
import hashlib
import json
import unicodedata
from pathlib import Path
from typing import Any

import pytest
from hypothesis import given
from hypothesis import strategies as st

from er.ingest.hashing import TOMBSTONE_CONTENT_HASH, UNIT_SEPARATOR, content_hash

REPO_ROOT = Path(__file__).resolve().parents[2]
HASHING_SOURCE = REPO_ROOT / "src" / "er" / "ingest" / "hashing.py"
VECTOR_FILE = REPO_ROOT / "tests" / "fixtures" / "content_hash_vectors.json"

# S4.1's dependency argument, in executable form: the fixture linter, the synthetic
# generator and the dbt-side oracle all import this digest, so anything outside this
# set puts config or lake code — and therefore a connection — in their import path.
ALLOWED_IMPORTS = {"hashlib", "unicodedata", "typing", "collections.abc"}

# The scenarios the ticket requires the file to cover. Named here so deleting a
# vector fails a test rather than quietly shrinking the corpus.
REQUIRED_VECTORS = {
    "ascii_three_columns",
    "extra_unmapped_keys",
    "column_order_swapped",
    "nfd_spelling",
    "nfc_spelling",
    "null_value",
    "empty_string_value",
    "single_column",
    "separator_inside_left_value",
    "separator_inside_right_value",
    "multibyte_non_latin",
}

VECTOR_DOC: dict[str, Any] = json.loads(VECTOR_FILE.read_text(encoding="utf-8"))
VECTORS: list[dict[str, Any]] = VECTOR_DOC["vectors"]
BY_NAME: dict[str, dict[str, Any]] = {vector["name"]: vector for vector in VECTORS}


def oracle(vector: dict[str, Any]) -> str:
    """Recompute a vector's digest from its written-out preimage, not from the module."""
    return hashlib.sha256(bytes.fromhex(vector["preimage_hex"])).hexdigest()


def module_imports() -> set[str]:
    """Top-level module names imported by ``src/er/ingest/hashing.py``."""
    tree = ast.parse(HASHING_SOURCE.read_text(encoding="utf-8"))
    names: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            names.update(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom):
            # `level > 0` is a relative import; record it as the empty name so the
            # assertion below rejects it rather than silently skipping it.
            names.add("" if node.level else (node.module or ""))
    return names


@pytest.mark.parametrize("vector", VECTORS, ids=[vector["name"] for vector in VECTORS])
def test_every_committed_vector_matches_independent_oracle(vector: dict[str, Any]) -> None:
    """AC1: oracle == committed digest == module output, for every vector."""
    name = vector["name"]
    computed = content_hash(vector["row"], vector["columns"])
    assert oracle(vector) == vector["digest"], f"{name}: preimage_hex does not hash to digest"
    assert computed == vector["digest"], f"{name}: module disagrees with the committed digest"


def test_vector_file_covers_the_required_scenarios() -> None:
    """The corpus is at least eight vectors and covers every scenario the ticket names."""
    assert len(VECTORS) >= 8
    assert {vector["name"] for vector in VECTORS} == REQUIRED_VECTORS
    assert len(BY_NAME) == len(VECTORS), "vector names must be unique"
    assert VECTOR_DOC["separator_hex"] == UNIT_SEPARATOR.encode("utf-8").hex()


def test_nfc_normalization_collapses_nfd() -> None:
    """AC2: the NFD and NFC spellings differ in the file and share one digest."""
    nfd, nfc = BY_NAME["nfd_spelling"], BY_NAME["nfc_spelling"]
    nfd_value = nfd["row"]["given_name"]
    nfc_value = nfc["row"]["given_name"]

    assert nfd_value != nfc_value, "the pair must be committed with distinct row bytes"
    assert unicodedata.normalize("NFC", nfd_value) == nfc_value
    assert nfd["digest"] == nfc["digest"]
    assert content_hash(nfd["row"], nfd["columns"]) == content_hash(nfc["row"], nfc["columns"])


def test_null_and_empty_string_share_an_encoding() -> None:
    """AC3: NULL and '' both encode as the empty string, so they collide by design."""
    null_vector, empty_vector = BY_NAME["null_value"], BY_NAME["empty_string_value"]
    assert null_vector["row"]["last_name"] is None
    assert empty_vector["row"]["last_name"] == ""
    assert null_vector["digest"] == empty_vector["digest"]

    columns = ["first_name", "last_name"]
    assert content_hash({"first_name": "Ada", "last_name": None}, columns) == content_hash(
        {"first_name": "Ada", "last_name": ""}, columns
    )
    # A column absent from the row is the same NULL: a source row that omits an
    # optional column must not hash differently from one that delivers it empty.
    assert content_hash({"first_name": "Ada"}, columns) == null_vector["digest"]


def test_column_order_is_load_bearing() -> None:
    """AC4: transposing two columns whose values differ changes the digest."""
    plain, swapped = BY_NAME["ascii_three_columns"], BY_NAME["column_order_swapped"]
    assert plain["row"] == swapped["row"]
    assert plain["columns"] != swapped["columns"]
    assert plain["digest"] != swapped["digest"]
    assert content_hash(plain["row"], swapped["columns"]) == swapped["digest"]


def test_excluded_fields_do_not_affect_digest() -> None:
    """AC5: keys outside `columns` — the S4.1 exclusions included — never reach the preimage."""
    plain, extra = BY_NAME["ascii_three_columns"], BY_NAME["extra_unmapped_keys"]
    assert plain["digest"] == extra["digest"]
    assert set(extra["row"]) > set(plain["row"])

    augmented = dict(plain["row"])
    augmented.update(
        ingested_at="2099-01-01T00:00:00Z",
        ingest_batch_id="01JZZZZZZZZZZZZZZZZZZZZZZZ",
        std_version="v99",
    )
    assert content_hash(augmented, plain["columns"]) == plain["digest"]


def test_separator_inside_a_value_is_hashed_verbatim() -> None:
    """AC8: a value containing 0x1f is not escaped, and the file records the ambiguity."""
    left, right = BY_NAME["separator_inside_left_value"], BY_NAME["separator_inside_right_value"]
    assert UNIT_SEPARATOR in left["row"]["addr1"]
    assert UNIT_SEPARATOR in right["row"]["addr_city"]
    assert left["row"] != right["row"]
    # Verbatim, therefore ambiguous: the two distinct rows share one preimage.
    assert left["preimage_hex"] == right["preimage_hex"]
    assert content_hash(left["row"], left["columns"]) == content_hash(
        right["row"], right["columns"]
    )
    assert "0x1f" in left["note"] and "ambiguity" in left["note"]


@given(
    st.dictionaries(
        keys=st.text(min_size=1, max_size=12),
        values=st.one_of(st.none(), st.text(max_size=32)),
        min_size=1,
        max_size=6,
    )
)
def test_tombstone_sentinel_is_unreachable(row: dict[str, str | None]) -> None:
    """AC6: no non-empty column list over any row produces the S4.1.1 sentinel."""
    assert TOMBSTONE_CONTENT_HASH == "0" * 64
    assert content_hash(row, list(row)) != TOMBSTONE_CONTENT_HASH


def test_module_imports_only_stdlib() -> None:
    """AC7: the module's import surface is a subset of the four stdlib names S4.1 allows."""
    assert module_imports() <= ALLOWED_IMPORTS


def test_exactly_one_content_hash_implementation() -> None:
    """S4.1 pins the digest to ONE function; a re-export is fine, a second `def` is not."""
    definitions = []
    for path in sorted((REPO_ROOT / "src" / "er").rglob("*.py")):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and node.name == "content_hash":
                definitions.append(path.relative_to(REPO_ROOT))
    assert definitions == [Path("src/er/ingest/hashing.py")]
