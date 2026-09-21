from collections.abc import Iterator

import duckdb
import pytest

from er.lake.bulk import insert_batches, staged_rows


@pytest.mark.parametrize("count", [0, 1, 1023, 1024, 1025, 2050])
def test_bulk_preserves_values_order_and_bounded_consumption(count: int) -> None:
    with duckdb.connect() as connection:
        connection.execute("CREATE TABLE staged (seq BIGINT, value VARCHAR, weight DOUBLE)")

        def rows() -> Iterator[tuple[int, str | None, float]]:
            for index in range(count):
                # At the start of each new batch, all earlier batches are already loaded.
                if index and index % 1024 == 0:
                    assert connection.execute("SELECT count(*) FROM staged").fetchone()[0] == index
                yield index, None if index % 7 == 0 else "O'Neil 雪 \n \"", index / 7

        assert (
            insert_batches(
                connection,
                "INSERT INTO staged SELECT unnest(?::BIGINT[]), "
                "unnest(?::VARCHAR[]), unnest(?::DOUBLE[])",
                rows(),
                columns=3,
            )
            == count
        )
        actual = connection.execute("SELECT * FROM staged ORDER BY seq").fetchall()
        assert actual == [
            (i, None if i % 7 == 0 else "O'Neil 雪 \n \"", i / 7) for i in range(count)
        ]


def test_bulk_rejects_ragged_rows_instead_of_null_padding() -> None:
    with duckdb.connect() as connection:
        connection.execute("CREATE TABLE staged (a VARCHAR, b VARCHAR)")
        with pytest.raises(ValueError, match="number of columns"):
            insert_batches(
                connection,
                "INSERT INTO staged SELECT unnest(?::VARCHAR[]), unnest(?::VARCHAR[])",
                [("one", "two"), ("short",)],
                columns=2,
            )
        assert connection.execute("SELECT count(*) FROM staged").fetchone() == (0,)


def test_staged_rows_are_local_cleaned_up_and_do_not_commit() -> None:
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute("CREATE TABLE lake.main.written (id BIGINT, value VARCHAR)")
        connection.execute("USE lake")
        connection.execute("BEGIN")
        expected = [(i, None if i % 7 == 0 else "O'Neil 雪") for i in range(2050)]
        with staged_rows(
            connection, (("id", "BIGINT"), ("value", "VARCHAR")), iter(expected)
        ) as relation:
            assert relation.startswith("temp.main.")
            assert (
                connection.execute(f"SELECT * FROM {relation} ORDER BY id").fetchall() == expected
            )
            connection.execute(f"INSERT INTO lake.main.written SELECT * FROM {relation}")
        assert connection.execute("SELECT count(*) FROM lake.main.written").fetchone() == (2050,)
        assert not connection.execute(
            "SELECT table_name FROM duckdb_tables() WHERE table_name LIKE 'er_bulk_%'"
        ).fetchall()
        connection.execute("ROLLBACK")
        assert connection.execute("SELECT count(*) FROM lake.main.written").fetchone() == (0,)


def test_staging_cleanup_preserves_errors() -> None:
    with duckdb.connect() as connection:
        with pytest.raises(ValueError, match="number of columns"):
            with staged_rows(connection, (("id", "BIGINT"),), [(1,), (2, 3)]):
                pytest.fail("ragged input must not reach the merge")
        assert not connection.execute(
            "SELECT table_name FROM duckdb_tables() WHERE table_name LIKE 'er_bulk_%'"
        ).fetchall()
        connection.execute("CREATE TABLE written (id BIGINT NOT NULL)")
        connection.execute("BEGIN")
        with pytest.raises(duckdb.ConstraintException):
            with staged_rows(connection, (("id", "BIGINT"),), [(None,)]) as relation:
                connection.execute(f"INSERT INTO written SELECT * FROM {relation}")
        connection.execute("ROLLBACK")
        assert connection.execute("SELECT count(*) FROM written").fetchone() == (0,)
