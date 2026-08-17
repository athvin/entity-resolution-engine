"""The dbt half of T-BLK-1's derivation, over a key table authored to break it (S4.2).

`tests/helpers/pairs.py` is the only place either candidate pair set is derived, so what
it does with the rows nobody writes on purpose has to be pinned somewhere that does not
need a lake. The integration suite scores `base_10`, where `int_blocking_keys` is
correct by construction: it has no NULL key, no empty key and no duplicated row, so a
derivation that mishandled all three would stay green there and fail the first time a
scenario delivered a record with a missing component in a concatenated key.

One hand-built table, holding exactly the four cases S4.2's policy and its `DISTINCT`
requirement are about:

* a **NULL** `key_value`, on two records — which must not block them together. SQL's own
  equality already refuses to match NULL to NULL; the case is here because that is a
  property of the engine rather than of the helper, and a rewrite that reached for
  `is not distinct from` would silently start blocking on absence.
* an **empty-string** `key_value`, on two records — which SQL's equality WOULD match, and
  which S4.2 forbids on either side: an empty key blocks every record whose key
  expression concatenates a missing component.
* a **duplicated** `(key_type, key_value, record_key)` row — the multiplicity S5.0 makes
  a dbt test rather than an engine constraint, so it can exist.
* a **single-record** key group — a key that groups one record and therefore produces no
  pair at all, which is what stops "every key emits a pair" being an accidental truth of
  the fixture.

The table relaxes the `NOT NULL` that `REGISTRY['int_blocking_keys']` declares on
`key_value`, and only that: the column list, its order and its types come off S5's own
`TableSpec`. The relaxation is the point — with it enforced, the NULL case would be
refused by the storage engine and the helper's own handling of it would never run.
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import Final

import duckdb
import pytest
from helpers.pairs import (
    BLOCKING_KEYS_RELATION,
    SPLINK_LABEL,
    Pair,
    blocking_key_pair_rows,
    canonical_pairs_from_blocking_keys,
    pair_key_types,
    symmetric_difference_report,
)

from er.lake.model import REGISTRY, SCHEMA_QUALIFIER

#: The four records the table keys. `billing:9` sorts before every `crm:` key, so the
#: canonical ordering the helper asserts is not the order the rows are inserted in.
RECORD_A: Final = "crm:1"
RECORD_B: Final = "crm:2"
RECORD_C: Final = "crm:3"
RECORD_D: Final = "billing:9"

#: The three `key_type`s the table uses, spelled here rather than imported from a config:
#: what is under test is the self-join, and it is indifferent to which rules produced the
#: rows (S4.2).
EMAIL: Final = "email_exact"
PHONE: Final = "phone_exact"
POSTAL: Final = "name_postal"
SOLO: Final = "dob_name"

_SHARED_EMAIL: Final = "a@example.com"
_SHARED_PHONE: Final = "+14155550132"

#: The table, as `(key_type, key_value, record_key)`. Two rows are the same triple —
#: that is the duplicate, and it is written out twice rather than inserted in a loop so
#: the reason the row count exceeds the pair count is readable here.
KEY_ROWS: Final[tuple[tuple[str, str | None, str], ...]] = (
    (EMAIL, _SHARED_EMAIL, RECORD_A),
    (EMAIL, _SHARED_EMAIL, RECORD_B),
    (EMAIL, _SHARED_EMAIL, RECORD_B),
    (PHONE, _SHARED_PHONE, RECORD_A),
    (PHONE, _SHARED_PHONE, RECORD_B),
    (PHONE, _SHARED_PHONE, RECORD_D),
    (EMAIL, None, RECORD_C),
    (EMAIL, None, RECORD_D),
    (POSTAL, "", RECORD_C),
    (POSTAL, "", RECORD_A),
    (SOLO, "1990-01-01|smith", RECORD_C),
)

#: What the derivation must return: the three pairs the two usable keys imply, and
#: nothing from the NULL, the empty string or the one-record group. `RECORD_C` appears in
#: no pair, which is the whole claim of the last three rows above.
EXPECTED_PAIRS: Final[frozenset[Pair]] = frozenset(
    {
        (RECORD_D, RECORD_A),
        (RECORD_D, RECORD_B),
        (RECORD_A, RECORD_B),
    }
)

#: The pair the duplicate row and the second `key_type` both carry. It is what the
#: `SELECT DISTINCT` of S4.2 collapses: the self-join reaches it three times.
MULTIPLY_CARRIED: Final[Pair] = (RECORD_A, RECORD_B)
MULTIPLY_CARRIED_ROWS: Final = 3


def relaxed_ddl() -> str:
    """`int_blocking_keys` as S5 declares it, minus the `NOT NULL` on `key_value`.

    Built from the registry's own `Column` objects rather than typed out, so a relation
    that grows a column reaches this fixture instead of leaving it asserting over a
    stale shape. Only the constraint is dropped, and only because the case under test is
    a value the constraint would refuse.
    """
    spec = REGISTRY[BLOCKING_KEYS_RELATION]
    body = ",\n".join(f"  {column.name} {column.type}" for column in spec.columns)
    return f"CREATE TABLE {SCHEMA_QUALIFIER}.{BLOCKING_KEYS_RELATION} (\n{body}\n)"


@pytest.fixture
def lake() -> Iterator[duckdb.DuckDBPyConnection]:
    """An in-memory stand-in for the lake, holding the hand-built key table.

    ATTACHed under the alias rather than created as `main.int_blocking_keys`: every
    statement in the module under test is written `lake.main.…` because S4.0b forbids
    DuckLake being the default catalog, and a fixture that made the table reachable
    unqualified would let a missing qualifier pass here and fail against a real lake.
    """
    connection = duckdb.connect()
    connection.execute("ATTACH ':memory:' AS lake")
    connection.execute(relaxed_ddl())
    connection.executemany(
        f"INSERT INTO {SCHEMA_QUALIFIER}.{BLOCKING_KEYS_RELATION} "
        f"(key_type, key_value, record_key, source_system, source_record_id) "
        f"VALUES (?, ?, ?, ?, ?)",
        [
            [key_type, key_value, record_key, *record_key.split(":", 1)]
            for key_type, key_value, record_key in KEY_ROWS
        ],
    )
    try:
        yield connection
    finally:
        connection.close()


def test_pair_derivation_over_hand_built_key_table(lake: duckdb.DuckDBPyConnection) -> None:
    """AC7: the four authored cases, the DISTINCT, the canonical ordering, the report."""
    pairs = canonical_pairs_from_blocking_keys(lake)
    rows = blocking_key_pair_rows(lake)

    assert pairs == set(EXPECTED_PAIRS), symmetric_difference_report(
        pairs, set(EXPECTED_PAIRS), key_types=pair_key_types(lake), b_label="expected"
    )

    # The NULL and the empty-string keys never block: `RECORD_C` carries one of each and
    # a key group of its own, and it reaches no pair by any of the three (S4.2).
    assert not [pair for pair in pairs if RECORD_C in pair]

    # Canonical and self-free, which the join expresses as `a.record_key < b.record_key`
    # and the helper re-asserts through the S5.0 canonicalisation (D9).
    for rec_a_key, rec_b_key in pairs:
        assert rec_a_key < rec_b_key, f"{rec_a_key} !< {rec_b_key}"

    # The DISTINCT is doing work: the self-join reaches the shared pair three times —
    # twice through the duplicated `email_exact` row and once through `phone_exact` — and
    # the derived set holds it once (S4.2).
    carried = [row for row in rows if (row[0], row[1]) == MULTIPLY_CARRIED]
    assert len(carried) == MULTIPLY_CARRIED_ROWS, carried
    assert len(rows) > len(pairs), "the un-DISTINCTed join collapsed on its own"
    assert sum(1 for pair in pairs if pair == MULTIPLY_CARRIED) == 1

    attribution = pair_key_types(lake)
    assert attribution[MULTIPLY_CARRIED] == (EMAIL, PHONE)
    assert set(attribution) == set(EXPECTED_PAIRS)


def test_report_names_the_missing_pairs_and_their_key_types(
    lake: duckdb.DuckDBPyConnection,
) -> None:
    """The failure message a T-BLK-1 divergence prints: pairs, sides, `key_type`s."""
    pairs = canonical_pairs_from_blocking_keys(lake)
    attribution = pair_key_types(lake)

    agreed = symmetric_difference_report(pairs, pairs, key_types=attribution)
    assert "equal" in agreed
    assert MULTIPLY_CARRIED[0] not in agreed, "an agreeing report should list no pairs"

    # One rule's worth of pairs dropped from the first side, which is the shape of the
    # divergence a lost `UNION ALL` branch produces.
    phone_only: set[Pair] = {
        pair for pair, types in attribution.items() if tuple(types) == (PHONE,)
    }
    assert phone_only, "the fixture has no pair carried by phone_exact alone"

    report = symmetric_difference_report(pairs - phone_only, pairs, key_types=attribution)
    lines = report.splitlines()
    assert f"only in {SPLINK_LABEL} ({len(phone_only)}):" in report
    for rec_a_key, rec_b_key in phone_only:
        located = [line for line in lines if rec_a_key in line and rec_b_key in line]
        assert located, f"{rec_a_key} | {rec_b_key} is missing from:\n{report}"
        assert f"key_type={PHONE}" in located[0], located
