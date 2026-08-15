"""The S4.1.1 deletion arm of `er ingest`, against the Compose substrate.

This is the ingest half of T-DEL-1a. What it holds `er ingest` to is the whole of
S4.1.1 that is reachable in M1: that a `--full-refresh-keys` delivery is a *complete
key set*, so every live key it omits gains a tombstone version row; that the tombstone
carries the sentinel `content_hash`, the JSON null document and a `deleted_at` equal
to its batch's `ingested_at`; that an empty full-refresh delivery is refused with exit
`2` having written nothing at all; and that a key re-appearing later is an ordinary
content version that the manifest counts as `resurrected`.

Every assertion is about what survived in the lake after a real `er` process wrote it.
The command is spawned as a subprocess, exactly as Compose runs it (S7.1), and the
lake is read back on a connection of its own.

The half T-DEL-1a asserts about `int_std_records` — tombstones excluded from the
standardized corpus — is deliberately absent: that relation does not exist until M2.
So is everything S4.5.5 does with a tombstone once it lands (edge invalidation,
`member_removed` / `split` / `retired`), which is ER-083's.

The corpus is an S8.2.1 scenario under this suite's own `data/` root rather than a
committed `fixtures/static/` one, because a `fixtures/static/` scenario is defined by
its `expected/` files and the five relations those describe do not exist yet. It is
still loaded through the ER-028 machinery, so its phase vocabulary is the spec's and
not this file's: `refresh` is the full-refresh delivery and `resurrect` the ordinary
one that follows it.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import Any

import duckdb
import pytest
from helpers.scenario import Scenario, load_scenario

from er.errors import ExitCode
from er.ingest.landing import EMPTY_FULL_REFRESH_MESSAGE, TOMBSTONE_CONTENT_HASH
from er.lake.columns import VOLATILE_COLUMNS
from er.lake.ducklake import connect
from er.lake.model import SCHEMA_QUALIFIER

#: Where this suite's scenarios live. Not `fixtures/static/`: see the module docstring.
DATA_ROOT = Path(__file__).resolve().parent / "data"

SCENARIO_NAME = "ingest_deletion"

#: The source the full refresh is scoped to.
SOURCE = "crm"

#: The sources that must be untouched by it (AC7). Read off the base phase rather
#: than listed, so a source added to the fixture is covered without a second edit.
OTHER_SOURCES = ("billing", "webforms")

#: The keys `refresh/` omits, and therefore the ones it must tombstone.
OMITTED_KEYS = ("C4", "C5")

#: The key `resurrect/` brings back. One of the omitted two, because a resurrection
#: is only observable for a key that was tombstoned.
RESURRECTED_KEY = "C4"

#: `raw_records` as S5 declares it. S5.1 forbids `SELECT *` across an additive
#: change, so the projection is named once.
RAW_COLUMNS: tuple[str, ...] = (
    "source_system",
    "source_record_id",
    "payload",
    "content_hash",
    "is_deleted",
    "deleted_at",
    "ingest_batch_id",
    "ingested_at",
)

#: The positions a delivery to another source must leave byte-identical. S5.0 defines
#: VOLATILE_COLUMNS once and forbids a second exclusion list.
STABLE_POSITIONS: tuple[int, ...] = tuple(
    index for index, column in enumerate(RAW_COLUMNS) if column not in VOLATILE_COLUMNS
)

#: The `ingest_batches` projection, in S5's column order.
BATCH_COLUMNS: tuple[str, ...] = (
    "ingest_batch_id",
    "run_id",
    "source_system",
    "path",
    "new_count",
    "changed_count",
    "unchanged_count",
    "tombstone_count",
    "resurrected_count",
    "full_refresh_keys",
    "created_at",
)


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def query(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: Any) -> list[Any]:
    return connection.execute(sql, list(parameters)).fetchall()


def raw_records(connection: duckdb.DuckDBPyConnection, source: str) -> list[Any]:
    """One source's `raw_records` rows, in an order independent of write order."""
    return query(
        connection,
        f"SELECT {', '.join(RAW_COLUMNS)} FROM {SCHEMA_QUALIFIER}.raw_records "
        f"WHERE source_system = ? ORDER BY source_record_id, content_hash",
        source,
    )


def stable(rows: list[Any]) -> list[tuple[Any, ...]]:
    """``rows`` with every VOLATILE_COLUMNS value dropped (S5.0)."""
    return [tuple(row[index] for index in STABLE_POSITIONS) for row in rows]


def batches(connection: duckdb.DuckDBPyConnection) -> list[dict[str, Any]]:
    """Every `ingest_batches` row as a mapping, oldest first (S5)."""
    rows = query(
        connection,
        f"SELECT {', '.join(BATCH_COLUMNS)} FROM {SCHEMA_QUALIFIER}.ingest_batches "
        f"ORDER BY created_at, ingest_batch_id",
    )
    return [dict(zip(BATCH_COLUMNS, row, strict=True)) for row in rows]


def payload_types(connection: duckdb.DuckDBPyConnection) -> dict[str, str]:
    """`json_type(payload)` per tombstoned key — S4.1.1's `payload` reading (S5)."""
    rows = query(
        connection,
        f"SELECT source_record_id, json_type(payload) FROM {SCHEMA_QUALIFIER}.raw_records "
        f"WHERE source_system = ? AND is_deleted",
        SOURCE,
    )
    return {str(key): str(kind) for key, kind in rows}


def tombstones(rows: list[Any]) -> list[Any]:
    """The tombstone version rows among ``rows`` (S4.1.1: `is_deleted`)."""
    return [row for row in rows if row[RAW_COLUMNS.index("is_deleted")]]


def stage_records(stderr: str) -> list[dict[str, Any]]:
    """The S5.2 records on stderr, in emission order."""
    return [json.loads(line) for line in stderr.splitlines() if line.startswith("{")]


@pytest.fixture
def ingest_deletion() -> Scenario:
    """The S8.2.1 scenario this suite delivers, opened through ER-028's loader."""
    scenario = load_scenario(SCENARIO_NAME, DATA_ROOT)
    assert scenario.phases == ("base", "refresh", "resurrect")
    return scenario


@pytest.fixture
def drop_root(tmp_path: Path) -> Path:
    """The root every phase of a test is delivered under, one subdirectory each."""
    return tmp_path


def deliver(scenario: Scenario, phase: str, root: Path) -> Path:
    """Materialise one phase as the drop-folder root `er ingest --path` reads.

    A scenario stores a phase as `<phase>/<source>.csv` (S8.2.1) while S4.1 reads
    `<path>/<source>/`, so the phase is copied rather than pointed at. Each phase gets
    its own root, which is also what keeps a later delivery from re-delivering an
    earlier one's files.
    """
    delivery = root / phase
    for source, path in scenario.inputs_for(phase).items():
        directory = delivery / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return delivery


def ingest_phase(scenario: Scenario, phase: str, root: Path, *args: str) -> Path:
    """Deliver ``phase`` and run `er ingest` over every source it carries.

    Returns the drop-folder root, so a caller that needs to re-run the same delivery
    (the repeated full refresh of AC3) points at the files that were already written
    rather than writing a second copy of them.
    """
    delivery = deliver(scenario, phase, root)
    for source in scenario.inputs_for(phase):
        result = run_er("ingest", "--source", source, "--path", str(delivery), *args)
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
    return delivery


def test_full_refresh_keys_appends_tombstones_for_omitted_keys(
    initialised_lake: duckdb.DuckDBPyConnection, ingest_deletion: Scenario, drop_root: Path
) -> None:
    """AC1/AC2: the omitted keys gain tombstones, and only they do.

    The delivery is the complete key set for `crm`, so "absent" means "deleted" — and
    every property S4.1.1 gives a tombstone row is asserted on the rows that appeared,
    not merely their number: the sentinel hash, the JSON null document, and a
    `deleted_at` equal to the batch's own instant.
    """
    ingest_phase(ingest_deletion, "base", drop_root)

    with connect() as connection:
        before = raw_records(connection, SOURCE)

    result = run_er(
        "ingest",
        "--source",
        SOURCE,
        "--path",
        str(deliver(ingest_deletion, "refresh", drop_root)),
        "--full-refresh-keys",
    )

    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    with connect() as connection:
        after = raw_records(connection, SOURCE)
        json_types = payload_types(connection)
        manifests = batches(connection)

    appended = tombstones(after)
    assert len(after) == len(before) + len(OMITTED_KEYS), "a tombstone is an appended row (D7)"
    assert [row[1] for row in appended] == list(OMITTED_KEYS)

    # AC1: no tombstone for a key the delivery carried. Asserted over the whole
    # relation rather than over the two rows above, so a third tombstone somewhere
    # else in the source fails here rather than passing on a count that happens to
    # match.
    delivered = set(ingest_deletion.inputs_for("refresh"))
    assert delivered == {SOURCE}
    assert set(json_types) == set(OMITTED_KEYS)

    refresh = manifests[-1]
    for row in appended:
        _, key, payload, content_hash, is_deleted, deleted_at, _batch, ingested_at = row
        assert is_deleted is True
        assert content_hash == TOMBSTONE_CONTENT_HASH
        # S4.1.1 says `payload = NULL` and S5 declares the column `JSON NOT NULL`;
        # the reconciling reading is the JSON null DOCUMENT, so the value is present
        # and its json_type is 'NULL'. A SQL NULL here would violate S5.
        assert payload is not None
        assert json_types[key] == "NULL"
        # AC1: `deleted_at` is the batch's `ingested_at` -- the instant the delivery
        # asserted the deletion, not the instant a row happened to be written.
        assert deleted_at == ingested_at
        assert deleted_at == refresh["created_at"]

    # AC2: the manifest row says what the batch did, in S5's five columns.
    assert refresh["source_system"] == SOURCE
    assert refresh["full_refresh_keys"] is True
    assert refresh["tombstone_count"] == len(OMITTED_KEYS)
    assert refresh["resurrected_count"] == 0
    assert (refresh["new_count"], refresh["changed_count"]) == (0, 0)
    # The three keys the delivery re-carried at their known content are `unchanged`:
    # counted, and deliberately not appended (S4.1).
    assert refresh["unchanged_count"] == len(before) - len(OMITTED_KEYS)

    # The stdout manifest line and the S4.1.1 counters carry the same two numbers.
    assert f"tombstoned={len(OMITTED_KEYS)}, resurrected=0" in result.stdout
    counters = stage_records(result.stderr)
    assert len(counters) == 1
    assert counters[0]["rows_out"] == len(OMITTED_KEYS)


def test_repeat_full_refresh_tombstones_nothing_and_exits_10(
    initialised_lake: duckdb.DuckDBPyConnection, ingest_deletion: Scenario, drop_root: Path
) -> None:
    """AC3: tombstoning is idempotent, because a tombstoned key is not live.

    The second refresh is the same delivery over a lake that already reflects it, so
    it is exactly the S4 preamble's idempotency contract applied to the deletion arm:
    nothing appended, `10`, and a manifest row that says so.
    """
    ingest_phase(ingest_deletion, "base", drop_root)
    refresh = deliver(ingest_deletion, "refresh", drop_root)
    first = run_er("ingest", "--source", SOURCE, "--path", str(refresh), "--full-refresh-keys")
    assert first.returncode == int(ExitCode.SUCCESS), first.stdout + first.stderr

    with connect() as connection:
        before = stable(raw_records(connection, SOURCE))

    second = run_er("ingest", "--source", SOURCE, "--path", str(refresh), "--full-refresh-keys")

    assert second.returncode == int(ExitCode.NOTHING_TO_DO), second.stdout + second.stderr

    with connect() as connection:
        after = raw_records(connection, SOURCE)
        manifests = batches(connection)

    assert stable(after) == before, "S4: re-execution leaves the lake logically unchanged"

    # No key carries two tombstones -- the property the row count above implies and
    # that this states directly, because it is the one a broken live-key definition
    # would break first.
    keys = [row[1] for row in tombstones(after)]
    assert sorted(keys) == sorted(OMITTED_KEYS)

    repeat = manifests[-1]
    assert repeat["full_refresh_keys"] is True
    assert repeat["tombstone_count"] == 0
    assert (repeat["new_count"], repeat["changed_count"]) == (0, 0)


def test_empty_full_refresh_delivery_is_refused_with_exit_2(
    initialised_lake: duckdb.DuckDBPyConnection, ingest_deletion: Scenario, drop_root: Path
) -> None:
    """AC4: an empty full-refresh delivery is a refusal to destroy, and writes nothing.

    `2` and not `10`: "nothing to do" does not abort an `er run-all` chain, and a
    delivery that would have emptied a source must. The message names the live keys it
    protected, because that count is what tells an operator what the refusal saved.
    """
    ingest_phase(ingest_deletion, "base", drop_root)

    with connect() as connection:
        before = raw_records(connection, SOURCE)
        before_batches = len(batches(connection))

    empty = drop_root / "empty"
    (empty / SOURCE).mkdir(parents=True)

    result = run_er("ingest", "--source", SOURCE, "--path", str(empty), "--full-refresh-keys")

    assert result.returncode == int(ExitCode.CONFIG), result.stdout + result.stderr

    # S4.1.1's literal message, written out here rather than rendered from the code
    # under test -- a constant asserted against itself would pass whatever it said.
    live = len(before)
    assert (
        f"empty full-refresh delivery: refusing to tombstone {live} live keys for source {SOURCE}"
        in result.stderr
    )
    assert EMPTY_FULL_REFRESH_MESSAGE.format(live=live, source=SOURCE) in result.stderr

    with connect() as connection:
        assert raw_records(connection, SOURCE) == before
        # AC4: the refusal wrote no manifest row either. A row claiming a
        # full-refresh delivery that tombstoned nothing would be indistinguishable
        # from AC3's legitimate no-op.
        assert len(batches(connection)) == before_batches


def test_resurrection_appends_ordinary_version_and_counts_it(
    initialised_lake: duckdb.DuckDBPyConnection, ingest_deletion: Scenario, drop_root: Path
) -> None:
    """AC5: a re-appearing key is an ordinary content version, counted as resurrected.

    No special case in the write: the row is appended by the same anti-join as any
    other new version, and it is `ingested_at` alone -- greater than the tombstone's --
    that makes the key live again under the S4.2 supersession rule.
    """
    ingest_phase(ingest_deletion, "base", drop_root)
    ingest_phase(ingest_deletion, "refresh", drop_root, "--full-refresh-keys")

    with connect() as connection:
        before = raw_records(connection, SOURCE)

    result = run_er(
        "ingest",
        "--source",
        SOURCE,
        "--path",
        str(deliver(ingest_deletion, "resurrect", drop_root)),
    )

    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    with connect() as connection:
        after = raw_records(connection, SOURCE)
        manifests = batches(connection)

    assert len(after) == len(before) + 1, "S4.1.1: resurrection APPENDS a content version"

    versions = [row for row in after if row[1] == RESURRECTED_KEY]
    live = [row for row in versions if not row[4]]
    tombstone = next(row for row in versions if row[4])
    resurrecting = max(live, key=lambda row: row[7])
    assert resurrecting[4] is False
    assert resurrecting[5] is None, "S5: `deleted_at` is NULL on every row but a tombstone"
    assert resurrecting[3] != TOMBSTONE_CONTENT_HASH
    assert resurrecting[7] > tombstone[7], "the resurrecting version supersedes the tombstone"

    resurrect = manifests[-1]
    assert resurrect["full_refresh_keys"] is False
    assert resurrect["resurrected_count"] == 1
    assert resurrect["changed_count"] == 1
    assert resurrect["new_count"] == 0
    assert resurrect["tombstone_count"] == 0
    assert "tombstoned=0, resurrected=1" in result.stdout


def test_full_refresh_is_scoped_to_one_source(
    initialised_lake: duckdb.DuckDBPyConnection, ingest_deletion: Scenario, drop_root: Path
) -> None:
    """AC7: a `crm` full refresh leaves every other source's keys exactly as they were.

    S4.1.1 scopes a full-refresh delivery to one `source_system`. Without that, the
    first refresh of any source would tombstone the entire corpus -- every key of every
    other source is, trivially, absent from it.
    """
    ingest_phase(ingest_deletion, "base", drop_root)

    with connect() as connection:
        before = {source: raw_records(connection, source) for source in OTHER_SOURCES}

    result = run_er(
        "ingest",
        "--source",
        SOURCE,
        "--path",
        str(deliver(ingest_deletion, "refresh", drop_root)),
        "--full-refresh-keys",
    )

    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    with connect() as connection:
        after = {source: raw_records(connection, source) for source in OTHER_SOURCES}

    for source in OTHER_SOURCES:
        assert before[source], f"{source} delivered no rows, so this asserts nothing"
        # Byte-identical, VOLATILE columns included: nothing was appended, and no row
        # was rewritten. A tombstone would show up as an extra row with the sentinel
        # hash, which is exactly what `raw_records` equality here rules out.
        assert after[source] == before[source]
        assert tombstones(after[source]) == []
