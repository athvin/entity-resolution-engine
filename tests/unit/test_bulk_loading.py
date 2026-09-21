from collections.abc import Iterator

import duckdb
import pytest

from er.lake.bulk import insert_batches


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
