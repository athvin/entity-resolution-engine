"""`name_variants` and the `nickname_variants` seed (DesignDoc.md S4.2, S4.3.1, S5; S8.4).

Three claims, and each is a precondition for something that is asserted at a layer
where it could otherwise only be observed rather than proved:

* **Element 0 is the record's own normalized `given_name`** (S4.2). `variant_match`
  is `ArrayIntersectLevel('name_variants', min_intersection=1)` (S4.3.1), and
  T-MATCH-SYM (S8.3) asserts `compare_two_records(a, b) == compare_two_records(b, a)`.
  Orientation-independence follows from the symmetry of the intersection only
  because every record contributes its own name to its own array; without it the
  integration assertion would pass or fail on which names happened to sort where.
* **The column is `LIST(VARCHAR) NOT NULL`** (S5). It is the empty list exactly
  when the name is NULL -- never SQL NULL, which the contract would reject and
  which `list_has_any` would answer NULL for.
* **The tail is sorted, de-duplicated and therefore byte-stable.** T-STD-1 (S8.3)
  hashes `array_to_string(name_variants, '\\x1f')` into `std_hash` and asserts it
  is unchanged across two `er standardize` runs, so two records with the same
  given name must produce the same bytes.

The seed is read from `dbt/seeds/nickname_variants.csv` and registered as a real
table, so these tests exercise the committed file rather than a fixture that
agrees with it today. That is also why this module builds its own harness: the
session harness in `conftest.py` registers nothing, and a test that mutates
harness state builds its own.
"""

from __future__ import annotations

import csv
import re
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from harness import MacroHarness
from hypothesis import given
from hypothesis import strategies as st

#: The seed S3 places at `dbt/seeds/`, from `tests/unit/dbt/test_name_variants.py`.
SEED_PATH = Path(__file__).resolve().parents[3] / "dbt" / "seeds" / "nickname_variants.csv"

SEED_NAME = "nickname_variants"

#: S8.2's nickname trap: `crm` "Robert Chen" and `webforms` "Bob Chen" MUST merge
#: through `variant_match`, and `bobby` is the same trap one hop further out.
NICKNAME_TRAP = (("bob", "robert"), ("bobby", "robert"))

#: AC7: the seed is exactly twelve rows of two lowercase ASCII values.
SEED_ROW_COUNT = 12
SEED_HEADER = ("variant_a", "variant_b")
LOWERCASE_ASCII = re.compile(r"\A[a-z]+\Z")

# Names, not free text: the alphabet is the one `name_norm` has rules for, so a
# generated value exercises the fold and the punctuation strip on its way in.
VARIANT_ALPHABET = " abzXYZ09.-_'áÅ中я"


def seed_pairs() -> list[tuple[str, str]]:
    """The committed seed as `(variant_a, variant_b)` tuples, in file order."""
    with SEED_PATH.open(encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle)
        return [(row["variant_a"], row["variant_b"]) for row in reader]


def seed_members() -> list[str]:
    """Every name the seed mentions, on either side of a pair."""
    return sorted({name for pair in seed_pairs() for name in pair})


@pytest.fixture(scope="module")
def seeded(dbt_vars: dict[str, Any]) -> Iterator[MacroHarness]:
    """A harness whose `nickname_variants` seed is the committed CSV."""
    harness = MacroHarness(vars=dbt_vars)
    harness.register_seed(
        SEED_NAME, [dict(zip(SEED_HEADER, pair, strict=True)) for pair in seed_pairs()]
    )
    try:
        yield harness
    finally:
        harness.close()


def variants(harness: MacroHarness, value: str | None) -> list[str]:
    result: list[str] = harness.eval_macro("name_variants", value)[0]["value"]
    return result


def normalize(harness: MacroHarness, value: str | None) -> str | None:
    result: str | None = harness.eval_macro("name_norm", value)[0]["value"]
    return result


def has_any(harness: MacroHarness, left: str | None, right: str | None) -> bool | None:
    """`list_has_any(name_variants(left), name_variants(right))`, as S4.3.1 uses it."""
    left_sql = harness.render_macro("name_variants", "pair.left_name")
    right_sql = harness.render_macro("name_variants", "pair.right_name")
    cursor = harness.execute(
        f"select list_has_any({left_sql}, {right_sql}) as hit from ("
        "select cast(? as varchar) as left_name, cast(? as varchar) as right_name"
        ") as pair",
        [left, right],
    )
    hit: bool | None = cursor.fetchone()[0]
    return hit


@given(st.text(alphabet=VARIANT_ALPHABET, max_size=12))
def test_normalized_name_is_element_zero(seeded: MacroHarness, value: str) -> None:
    """AC3: the S4.2 symmetry guarantee, over generated names."""
    name = normalize(seeded, value)
    array = variants(seeded, value)
    if name is None:
        assert array == []
        return
    # DuckDB lists are 1-indexed, so S4.2's element 0 is `name_variants[1]` in SQL
    # and `array[0]` here. Same element, two spellings of the index.
    assert array[0] == name


def test_never_null_and_empty_list_when_name_is_null(seeded: MacroHarness) -> None:
    """AC4: `LIST(VARCHAR) NOT NULL` (S5) -- empty exactly when the name is NULL."""
    for absent in (None, "", "   ", "NULL", "n/a", "-", "unknown"):
        assert normalize(seeded, absent) is None
        # Not `is None`: SQL NULL would violate the S5 contract and would make
        # `list_has_any` answer NULL instead of false.
        assert variants(seeded, absent) == []

    for present in ("Robert", "  ZOË  ", "O'Brien"):
        array = variants(seeded, present)
        assert array != []
        assert array[0] == normalize(seeded, present)


def test_robert_bob_bobby_intersect_in_both_orientations(seeded: MacroHarness) -> None:
    """AC5: `variant_match` fires either way round, and not on unrelated names."""
    for other in ("Bob", "Bobby"):
        assert has_any(seeded, "Robert", other) is True
        assert has_any(seeded, other, "Robert") is True

    # One hop, never a transitive closure: `bob` and `bobby` meet only at `robert`,
    # which `min_intersection=1` is enough to find without inflating the graph.
    assert variants(seeded, "Bob") == ["bob", "robert"]
    assert variants(seeded, "Bobby") == ["bobby", "robert"]

    assert has_any(seeded, "Robert", "Susan") is False
    assert has_any(seeded, "Susan", "Robert") is False
    # A name the seed never mentions still intersects itself, and nothing else.
    assert has_any(seeded, "Chen", "Chen") is True
    assert has_any(seeded, "Chen", "Robert") is False


def test_seed_closure_is_symmetric(seeded: MacroHarness) -> None:
    """AC6: a row `(a, b)` contributes `b` to `a` and `a` to `b`."""
    for variant_a, variant_b in seed_pairs():
        assert variant_b in variants(seeded, variant_a)
        assert variant_a in variants(seeded, variant_b)

    # AC3 over every seed member, which is where the guarantee has to hold if
    # T-MATCH-SYM is to be a theorem about the fixture rather than about text.
    for member in seed_members():
        assert variants(seeded, member)[0] == normalize(seeded, member) == member


def test_nickname_seed_lint(seeded: MacroHarness) -> None:
    """AC7: the committed seed's own shape, checked as data rather than trusted."""
    lines = SEED_PATH.read_text(encoding="utf-8").splitlines()
    assert lines[0] == ",".join(SEED_HEADER)

    pairs = seed_pairs()
    assert len(lines) == SEED_ROW_COUNT + 1  # no blank or duplicated header lines
    assert len(pairs) == SEED_ROW_COUNT
    assert len(set(pairs)) == SEED_ROW_COUNT  # no duplicate pair
    assert pairs == sorted(pairs)  # sorted ascending

    for variant_a, variant_b in pairs:
        assert LOWERCASE_ASCII.match(variant_a), variant_a
        assert LOWERCASE_ASCII.match(variant_b), variant_b
        # Canonical orientation, the same rule S5.0 puts on a record pair: one
        # row per unordered pair, so the symmetric read below cannot double-count.
        assert variant_a < variant_b

    for trap in NICKNAME_TRAP:
        assert trap in pairs

    # Every seed value is already in `name_norm`'s normal form, or the join at
    # query time would silently miss the row it was written for.
    for member in seed_members():
        assert normalize(seeded, member) == member


def test_tail_is_sorted_deduped_and_byte_stable(seeded: MacroHarness) -> None:
    """AC8: two records with the same given name yield byte-identical arrays."""
    for member in seed_members():
        array = variants(seeded, member)
        tail = array[1:]
        assert tail == sorted(set(tail))
        assert array[0] not in tail  # the name is contributed once, by position 0

    # `elizabeth` is the multi-neighbour case, so the sort is doing work here.
    assert variants(seeded, "Elizabeth") == ["elizabeth", "beth", "liz"]

    # Byte stability is what T-STD-1 hashes: the same name, spelled three ways
    # that `name_norm` folds together, must produce one array and one join.
    spellings = [variants(seeded, value) for value in ("Elizabeth", "  ELIZABETH ", "Élizabeth")]
    assert spellings[0] == spellings[1] == spellings[2]
    assert len({"\x1f".join(spelling) for spelling in spellings}) == 1
