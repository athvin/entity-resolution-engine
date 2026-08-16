"""``table_content_hash`` — the T-STD-1 digest, pinned (S8.3, S5.0, S8.4).

What this layer proves is the *encoding*, because the encoding is the only part of
T-STD-1 that a passing integration run cannot check: standardize twice and the hashes
agree whatever the rendering rules are, so a run that quietly began truncating
``updated_at_source`` to its day, or rendering ``False`` as ``0``, would stay green
while every committed ``expected/<phase>/std_hashes.csv`` (S8.2.1) went stale.

Four properties, one test each:

* **the golden vector** — two hand-authored rows, one NULL in every nullable position
  and one fully populated, hashing to digests committed here as literals. The inputs
  that produced them are in this file, so a deliberate re-pin is a visible diff.
* **the column list** — the eighteen names of S8.3 in order, disjoint from the
  ``VOLATILE_COLUMNS`` this test imports, and asserted to be imported rather than
  re-listed inside :mod:`er.lake.hashing`.
* **scalar rendering** — the whole preimage of the populated row spelled out, so each
  rule (BOOLEAN, DATE, TIMESTAMP, NULL, ``array_to_string``) is readable rather than
  implied by a digest, plus the mutation arm that shows each one is load-bearing.
* **volatile independence** — the run-scoped stamps are not in the preimage, which is
  the property M7 redefined standardization determinism around.
"""

from __future__ import annotations

import ast
import hashlib
from collections.abc import Mapping
from datetime import date, datetime
from pathlib import Path

import pytest

from er.ingest.hashing import UNIT_SEPARATOR
from er.lake import hashing
from er.lake.columns import STD_RECORD_COLUMNS, VOLATILE_COLUMNS
from er.lake.hashing import STD_HASH_COLUMNS, table_content_hash

#: S8.3's T-STD-1 projection, transcribed here from the spec table rather than
#: imported. The module under test is exactly what this file is checking, so reading
#: its own tuple back would assert only that a tuple equals itself.
T_STD_1_COLUMNS: tuple[str, ...] = (
    "record_key",
    "std_version",
    "given_name",
    "family_name",
    "name_variants",
    "email",
    "email_valid",
    "phone_e164",
    "phone_valid",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
    "updated_at_source",
    "content_hash",
)

#: AC1's first row: every nullable column of `int_std_records` NULL. `record_key`,
#: `std_version`, `name_variants` and `content_hash` are NOT NULL in S5, so they carry
#: values; `name_variants` carries the empty list, which is the NOT NULL column's own
#: empty case and renders to the empty string like a NULL neighbour.
NULLED_ROW: Mapping[str, object] = {
    "record_key": "crm:1001",
    "std_version": "1",
    "given_name": None,
    "family_name": None,
    "name_variants": [],
    "email": None,
    "email_valid": None,
    "phone_e164": None,
    "phone_valid": None,
    "addr_number": None,
    "addr_street": None,
    "addr_unit": None,
    "addr_city": None,
    "addr_region": None,
    "addr_postal": None,
    "birth_date": None,
    "updated_at_source": None,
    "content_hash": "ab" * 32,
}

#: AC1's second row: every column populated, and every S5 type on this relation
#: exercised — VARCHAR, `LIST(VARCHAR)`, both BOOLEAN flags (one of each value, so
#: `true` and `false` are both in the preimage), DATE and TIMESTAMP.
POPULATED_ROW: Mapping[str, object] = {
    "record_key": "billing:2002",
    "std_version": "1",
    "given_name": "bob",
    "family_name": "roberts",
    "name_variants": ["bob", "robert"],
    "email": "bob@example.com",
    "email_valid": True,
    "phone_e164": "+15555550123",
    "phone_valid": False,
    "addr_number": "742",
    "addr_street": "evergreen terrace",
    "addr_unit": "4b",
    "addr_city": "springfield",
    "addr_region": "or",
    "addr_postal": "97477",
    "birth_date": date(2001, 2, 3),
    "updated_at_source": datetime(2026, 1, 2, 3, 4, 5),
    "content_hash": "cd" * 32,
}

#: The committed golden vector. Re-pinning either literal re-pins every
#: `std_hashes.csv` in `fixtures/static/`, which is the point of committing them.
NULLED_DIGEST = "f8ae8b434815aed803ccf12128365a359e7b6e94fe3a0f3a05aa026e9dcc901a"
POPULATED_DIGEST = "8791232a2210e73e1af40c70d52bf241d63073361cf35fd2f5ceb820682cec21"

#: `POPULATED_ROW`'s preimage, one rendered value per T-STD-1 column, in order. This
#: is AC3's real assertion: the four rules it names are readable as literals here
#: (`true`, `false`, `2001-02-03`, `bob\x1frobert`) instead of being implied by a hex
#: digest nobody can read.
POPULATED_RENDERED: tuple[str, ...] = (
    "billing:2002",
    "1",
    "bob",
    "roberts",
    "bob\x1frobert",
    "bob@example.com",
    "true",
    "+15555550123",
    "false",
    "742",
    "evergreen terrace",
    "4b",
    "springfield",
    "or",
    "97477",
    "2001-02-03",
    "2026-01-02 03:04:05",
    "cd" * 32,
)

#: Two distinct values for every `VOLATILE_COLUMNS` member, as AC6's pair of rows
#: carries them. Both are plain strings: what the test asserts is that these keys never
#: reach the preimage at all, so their types are irrelevant to it.
VOLATILE_A: Mapping[str, object] = {column: "a" for column in sorted(VOLATILE_COLUMNS)}
VOLATILE_B: Mapping[str, object] = {column: "b" for column in sorted(VOLATILE_COLUMNS)}


def sha256_of(values: tuple[str, ...]) -> str:
    """The S4.1 digest of ``values``: joined by `0x1f`, UTF-8, SHA-256, lowercase hex."""
    return hashlib.sha256(UNIT_SEPARATOR.join(values).encode("utf-8")).hexdigest()


def string_constants(module_path: Path) -> list[str]:
    """Every string literal in ``module_path``, docstrings included."""
    tree = ast.parse(module_path.read_text(encoding="utf-8"))
    return [
        node.value
        for node in ast.walk(tree)
        if isinstance(node, ast.Constant) and isinstance(node.value, str)
    ]


def test_golden_vector_matches() -> None:
    """AC1: both hand-authored rows hash to the digests committed above."""
    assert table_content_hash(NULLED_ROW) == NULLED_DIGEST
    assert table_content_hash(POPULATED_ROW) == POPULATED_DIGEST

    # Non-vacuous in the one way a golden vector can be vacuous: two rows that hashed
    # alike would satisfy a pair of literals that were themselves equal.
    assert NULLED_DIGEST != POPULATED_DIGEST

    # The NULL row's preimage is seventeen separators around four values, which is the
    # encoding S8.3 pins: NULL is the empty string and nothing else.
    assert table_content_hash(NULLED_ROW) == sha256_of(
        ("crm:1001", "1", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "ab" * 32)
    )


def test_column_list_is_t_std_1_order_and_non_volatile() -> None:
    """AC2: S8.3's eighteen columns, in order, and none of them run-scoped."""
    assert STD_HASH_COLUMNS == T_STD_1_COLUMNS
    assert len(STD_HASH_COLUMNS) == 18

    # Order is the preimage, so equality of the tuples is the assertion and set
    # equality would pass a permutation that silently re-pins every committed hash.
    assert list(STD_HASH_COLUMNS) != sorted(STD_HASH_COLUMNS)

    assert set(STD_HASH_COLUMNS) & VOLATILE_COLUMNS == set()
    # Every member is a real `int_std_records` column: a typo would otherwise be
    # invisible until a row that lacked the misspelled key reached the function.
    assert set(STD_HASH_COLUMNS) <= set(STD_RECORD_COLUMNS)

    # AC2's second clause: the frozen set is IMPORTED, never restated. A local copy
    # would drift from S5.0 exactly once and then stay wrong, so the module may not
    # name a volatile column at all -- not in a literal, and not in its prose.
    source = Path(str(hashing.__file__))
    restated = sorted(set(string_constants(source)) & VOLATILE_COLUMNS)
    assert restated == [], f"{source.name} re-lists VOLATILE_COLUMNS members: {restated}"


def test_scalar_rendering_is_pinned() -> None:
    """AC3: each rendering rule is spelled out, and each one moves the digest."""
    assert table_content_hash(POPULATED_ROW) == sha256_of(POPULATED_RENDERED)

    # The four rules AC3 names, read off the preimage above rather than asserted
    # through a private helper: the rendering is only observable through the digest,
    # and this is the tuple the digest was taken over.
    rendered = dict(zip(STD_HASH_COLUMNS, POPULATED_RENDERED, strict=True))
    assert rendered["email_valid"] == "true"
    assert rendered["phone_valid"] == "false"
    assert rendered["birth_date"] == "2001-02-03"
    assert rendered["updated_at_source"] == "2026-01-02 03:04:05"
    assert rendered["name_variants"] == "bob\x1frobert"

    # Each rule is load-bearing: change the value the rule renders and the digest
    # moves. The mutations are chosen so none of them collides with the encoding --
    # a `None` -> `''` mutation is deliberately absent, because those two ARE equal.
    baseline = table_content_hash(POPULATED_ROW)
    mutations: dict[str, object] = {
        "email_valid": False,
        "phone_valid": True,
        "birth_date": date(2001, 3, 2),
        "updated_at_source": datetime(2026, 1, 2, 3, 4, 6),
        "name_variants": ["robert", "bob"],
    }
    for column, value in mutations.items():
        mutated = {**POPULATED_ROW, column: value}
        assert table_content_hash(mutated) != baseline, (
            f"{column}: rendering is not in the preimage"
        )

    # NULL renders as the empty string, so a NULL and an empty string in the same
    # position are indistinguishable at hash level. S8.3 pins that and forbids a
    # sentinel; it is asserted here so a later "fix" fails instead of re-pinning the
    # committed vectors silently.
    nulled_email = {**POPULATED_ROW, "email": None}
    empty_email = {**POPULATED_ROW, "email": ""}
    assert table_content_hash(nulled_email) == table_content_hash(empty_email)

    # And an unrenderable type is refused rather than coerced through `str()`, which
    # is what keeps a schema change from being absorbed into the preimage.
    with pytest.raises(TypeError):
        table_content_hash({**POPULATED_ROW, "addr_number": 742})


def test_volatile_only_difference_hashes_equal() -> None:
    """AC6: two rows differing only in `VOLATILE_COLUMNS` values hash identically."""
    assert VOLATILE_A != VOLATILE_B

    row_a = {**POPULATED_ROW, **VOLATILE_A}
    row_b = {**POPULATED_ROW, **VOLATILE_B}
    assert table_content_hash(row_a) == table_content_hash(row_b) == POPULATED_DIGEST

    # The stamps `int_std_records` actually carries (S5) are in that set, so the claim
    # is about this relation and not only about a nine-name abstraction.
    assert {"ingest_batch_id", "ingested_at"} <= set(VOLATILE_A)

    # A missing column is a projection bug, not a NULL: hashing it as one would make a
    # short projection agree with a complete row whose value happened to be NULL.
    incomplete = {column: value for column, value in row_a.items() if column != "email"}
    with pytest.raises(KeyError):
        table_content_hash(incomplete)
