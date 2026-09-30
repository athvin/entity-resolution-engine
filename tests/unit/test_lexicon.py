"""The nickname lexicon relation and its verbs (er.std.lexicon).

Covers the three properties the module's docstring claims: retractable rows in
the `assertions` shape, one semantic hash over the active set, and `er init`
seeding that treats a pre-lexicon dbt-seed table as rows to backfill rather
than rows to duplicate. The migration case runs through the real S5.1 machinery
— :func:`er.lake.ddl.evolve` adds the retractable columns to an old-shape table
— so migrated and freshly seeded lakes are proven hash-identical.
"""

from __future__ import annotations

from pathlib import Path

import duckdb
import pytest

from er.errors import ConfigError
from er.lake.ddl import evolve
from er.lake.model import REGISTRY, create_table_sql
from er.std.lexicon import (
    SEED_CREATED_BY,
    active_pairs,
    add_pair,
    lexicon_hash,
    load_pairs,
    packaged_seed_pairs,
    remove_pair,
    seed_lexicon,
)


@pytest.fixture
def lake() -> duckdb.DuckDBPyConnection:
    connection = duckdb.connect()
    connection.execute("ATTACH ':memory:' AS lake")
    connection.execute("CREATE SCHEMA IF NOT EXISTS lake.main")
    return connection


@pytest.fixture
def seeded(lake: duckdb.DuckDBPyConnection) -> duckdb.DuckDBPyConnection:
    lake.execute(create_table_sql(REGISTRY["nickname_variants"]))
    seed_lexicon(lake)
    return lake


def test_hash_is_none_only_before_the_relation_exists(
    lake: duckdb.DuckDBPyConnection,
) -> None:
    assert lexicon_hash(lake) is None
    lake.execute(create_table_sql(REGISTRY["nickname_variants"]))
    # An empty relation hashes to a value: absence of pairs is a state, not
    # absence of the lake.
    assert lexicon_hash(lake) is not None


def test_seeding_loads_the_packaged_pairs_once(
    seeded: duckdb.DuckDBPyConnection,
) -> None:
    assert active_pairs(seeded) == tuple(sorted(packaged_seed_pairs()))
    first = lexicon_hash(seeded)
    assert seed_lexicon(seeded) == 0, "a seeded lake is touched zero times"
    assert lexicon_hash(seeded) == first
    creators = seeded.execute(
        "SELECT DISTINCT created_by FROM lake.main.nickname_variants"
    ).fetchall()
    assert creators == [(SEED_CREATED_BY,)]


def test_migrated_dbt_seed_table_backfills_to_the_same_hash(
    seeded: duckdb.DuckDBPyConnection,
) -> None:
    # A second lake in the pre-lexicon shape: the dbt seed's two columns only.
    migrated = duckdb.connect()
    migrated.execute("ATTACH ':memory:' AS lake")
    migrated.execute("CREATE SCHEMA IF NOT EXISTS lake.main")
    migrated.execute(
        "CREATE TABLE lake.main.nickname_variants (variant_a VARCHAR, variant_b VARCHAR)"
    )
    migrated.executemany(
        "INSERT INTO lake.main.nickname_variants VALUES (?, ?)",
        [list(pair) for pair in packaged_seed_pairs()],
    )
    evolve(migrated)  # the S5.1 additive reconcile `er init` runs
    backfilled = seed_lexicon(migrated)
    assert backfilled == len(packaged_seed_pairs())
    assert lexicon_hash(migrated) == lexicon_hash(seeded)
    ids = migrated.execute(
        "SELECT count(*) FROM lake.main.nickname_variants WHERE variant_id IS NULL"
    ).fetchone()
    assert ids is not None and ids[0] == 0


def test_add_is_canonical_and_idempotent(seeded: duckdb.DuckDBPyConnection) -> None:
    before = lexicon_hash(seeded)
    # The packaged lexicon already holds (anthony, tony) — in either spelling,
    # any casing, this is nothing to do.
    assert add_pair(seeded, "Tony", "ANTHONY", created_by="s") is None
    assert lexicon_hash(seeded) == before

    written = add_pair(seeded, "Peggy", "margaret", created_by="s")
    assert written is not None
    assert (written.variant_a, written.variant_b) == ("margaret", "peggy")
    assert lexicon_hash(seeded) != before
    assert ("margaret", "peggy") in active_pairs(seeded)


def test_remove_retracts_and_readd_mints_a_new_row(
    seeded: duckdb.DuckDBPyConnection,
) -> None:
    seeded_hash = lexicon_hash(seeded)
    retracted = remove_pair(seeded, "robert", "bob", retracted_by="s")
    assert retracted is not None and retracted.active is False
    assert ("bob", "robert") not in active_pairs(seeded)
    assert lexicon_hash(seeded) != seeded_hash
    assert remove_pair(seeded, "bob", "robert", retracted_by="s") is None

    rows = seeded.execute(
        "SELECT count(*) FROM lake.main.nickname_variants WHERE variant_a='bob' "
        "AND variant_b='robert'"
    ).fetchone()
    assert rows is not None and rows[0] == 1, "retraction flips active, never deletes"

    readded = add_pair(seeded, "bob", "robert", created_by="s")
    assert readded is not None and readded.variant_id != retracted.variant_id
    assert lexicon_hash(seeded) == seeded_hash, "the semantic set is restored"


def test_load_is_bulk_idempotent_and_validated(
    seeded: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    path = tmp_path / "pairs.csv"
    path.write_text(
        "variant_a,variant_b\nbob,robert\nmeg,margaret\nMARGARET,meg\n", encoding="utf-8"
    )
    written = load_pairs(seeded, path, created_by="s")
    # bob/robert is already active; the two margaret spellings are one pair.
    assert [(p.variant_a, p.variant_b) for p in written] == [("margaret", "meg")]
    assert load_pairs(seeded, path, created_by="s") == ()

    bad = tmp_path / "bad.csv"
    bad.write_text("left,right\nbob,robert\n", encoding="utf-8")
    with pytest.raises(ConfigError, match="variant_a,variant_b"):
        load_pairs(seeded, bad, created_by="s")


def test_malformed_pairs_are_refused(seeded: duckdb.DuckDBPyConnection) -> None:
    with pytest.raises(ConfigError, match="non-empty"):
        add_pair(seeded, "  ", "bob", created_by="s")
    with pytest.raises(ConfigError, match="its own variant"):
        add_pair(seeded, "Bob", "bob", created_by="s")
