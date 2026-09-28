"""A genuinely new source is a config change alone (S4.2) — the feature, end to end.

The unified `stg_records` model renders one union arm per `sources:` entry, so a
fourth source that no shipped file ever named must flow ingest → standardize →
match → reconcile → assemble on the strength of its config block. This suite
proves exactly that with `partnerapp`: novel column headers, `priority_rank: 4`,
and a record that duplicates a `base_10` persona so the claim reaches all the way
to a CROSS-SOURCE entity — a partnerapp record co-clustered with the persona's
crm/billing/webforms records — rather than stopping at "it staged".

The removal arm is the same claim's other face: dropping a source from config
merely removes its union arm. Its already-staged rows persist inert and the next
standardize builds green — where v1's per-source models failed the whole build at
render time on `var('sources')['<gone>']`.
"""

from __future__ import annotations

import csv
import os
import shutil
import subprocess
from pathlib import Path

import duckdb
import pytest

from er.errors import ExitCode
from er.lake.ducklake import connect
from er.lake.model import SCHEMA_QUALIFIER

REPO_ROOT = Path(__file__).resolve().parents[3]

#: The genuinely new source: a name no dbt file, registry tuple or fixture names.
PARTNER_SOURCE = "partnerapp"

#: Novel headers, mapped only by the derived config block below.
PARTNER_HEADER = (
    "partner_id",
    "given",
    "surname",
    "mail",
    "tel",
    "street",
    "town",
    "province",
    "postal",
    "born",
    "touched_at",
)

#: PA1 duplicates base_10's P1 (crm C001: same name, email, phone, address, DOB),
#: so it must land in P1's entity. PA2 is a novel persona and must not.
PARTNER_ROWS = (
    (
        "PA1",
        "Robert",
        "Chen",
        "robert.chen@example.com",
        "(628) 555-0101",
        "1420 Judah Street Apt 5",
        "San Francisco",
        "CA",
        "94107",
        "1985-03-14",
        "2024-03-01 09:00:00",
    ),
    (
        "PA2",
        "Zelda",
        "Quintanilla",
        "zelda.quintanilla@example.com",
        "(415) 555-0888",
        "9 Distinct Way",
        "San Francisco",
        "CA",
        "94131",
        "1969-01-30",
        "2024-03-02 10:00:00",
    ),
)

#: The `sources:` block a wizard-published config would add — appended verbatim to
#: `configs/test.yaml`'s own block, which keeps every other setting identical.
PARTNER_BLOCK = """  partnerapp:
    adapter: csv
    priority_rank: 4
    record_id_column: partner_id
    updated_at_column: touched_at
    date_format: "%Y-%m-%d"
    columns:
      given_name:   given
      family_name:  surname
      email:        mail
      phone:        tel
      address_line: street
      addr_city:    town
      addr_region:  province
      addr_postal:  postal
      birth_date:   born
"""


def derived_config(tmp_path: Path) -> Path:
    """`configs/test.yaml` plus the partnerapp block: the post-publish document."""
    base = Path(os.environ["ER_CONFIG"]).read_text(encoding="utf-8")
    marker = "\nblocking:"
    assert marker in base, "configs/test.yaml no longer has a blocking: block"
    derived = tmp_path / "custom-source.yaml"
    derived.write_text(base.replace(marker, f"{PARTNER_BLOCK}{marker}", 1), encoding="utf-8")
    return derived


def deliver_partner_file(root: Path) -> None:
    directory = root / PARTNER_SOURCE
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / "partnerapp.csv").open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(PARTNER_HEADER)
        writer.writerows(PARTNER_ROWS)


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def query(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: object) -> list:
    return connection.execute(sql, list(parameters)).fetchall()


def entity_of(connection: duckdb.DuckDBPyConnection, record_key: str) -> str:
    rows = query(
        connection,
        f"SELECT entity_id FROM {SCHEMA_QUALIFIER}.entity_membership WHERE record_key = ?",
        record_key,
    )
    assert len(rows) == 1, f"{record_key}: expected exactly one membership, got {rows}"
    return str(rows[0][0])


@pytest.fixture
def base_corpus(initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path) -> Path:
    """`base_10` ingested and standardized, the fixture model registered.

    `prepare_cli_fixture` is the canonical CLI setup: three sources delivered
    to a drop root, `er ingest` per source, `er standardize`, and the
    pre-trained `model_test_v1` installed — everything a tenant has the moment
    a wizard publishes a fourth source.
    """
    import sys

    sys.path.insert(0, str(REPO_ROOT / "tests"))
    from helpers.cli_fixture import prepare_cli_fixture

    root = tmp_path / "drop"
    prepare_cli_fixture(initialised_lake, root)
    return root


def test_fourth_source_clusters_cross_source(base_corpus: Path, tmp_path: Path) -> None:
    """AC: config block + delivered file + one full run ⇒ cross-source entity."""
    config = derived_config(tmp_path)
    deliver_partner_file(base_corpus)

    delivered = run_er(
        "ingest", "--source", PARTNER_SOURCE, "--path", str(base_corpus), "--config", str(config)
    )
    assert delivered.returncode == int(ExitCode.SUCCESS), delivered.stdout + delivered.stderr

    resolved = run_er("run-all", "--mode", "full", "--skip-ingest", "--config", str(config))
    assert resolved.returncode == int(ExitCode.SUCCESS), resolved.stdout + resolved.stderr

    with connect() as connection:
        staged = query(
            connection,
            f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.stg_records WHERE source_system = ?",
            PARTNER_SOURCE,
        )
        assert staged == [(len(PARTNER_ROWS),)]

        current = query(
            connection,
            f"SELECT record_key FROM {SCHEMA_QUALIFIER}.int_std_records "
            f"WHERE source_system = ? ORDER BY record_key",
            PARTNER_SOURCE,
        )
        assert current == [(f"{PARTNER_SOURCE}:PA1",), (f"{PARTNER_SOURCE}:PA2",)]

        # PA1 duplicates P1, so it joins P1's entity — the cross-source claim.
        # C001 is P1's crm record; equality of entity ids is co-clustering.
        pa1_entity = entity_of(connection, f"{PARTNER_SOURCE}:PA1")
        assert pa1_entity == entity_of(connection, "crm:C001")

        # PA2 is a novel persona: its entity holds exactly one member.
        pa2_entity = entity_of(connection, f"{PARTNER_SOURCE}:PA2")
        assert pa2_entity != pa1_entity
        (pa2_members,) = query(
            connection,
            f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.entity_membership WHERE entity_id = ?",
            pa2_entity,
        )[0]
        assert pa2_members == 1

        # Both entities assembled: the run reached golden records, not just clusters.
        for entity_id in (pa1_entity, pa2_entity):
            (golden,) = query(
                connection,
                f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.golden_records WHERE entity_id = ?",
                entity_id,
            )[0]
            assert golden == 1, f"{entity_id}: no golden record assembled"

        # The winning email came from the partner record's own arm for PA2's
        # entity — the mapping worked, not merely the staging.
        (pa2_email,) = query(
            connection,
            f"SELECT email FROM {SCHEMA_QUALIFIER}.golden_records WHERE entity_id = ?",
            pa2_entity,
        )[0]
        assert pa2_email == "zelda.quintanilla@example.com"


def test_removed_source_leaves_rows_inert_and_builds_green(
    initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    """Removal arm: no render-time failure, no corruption, rows persist."""
    subprocess.run(["dbt", "deps", "--project-dir", "dbt"], capture_output=True, check=True)
    config = derived_config(tmp_path)
    root = tmp_path / "drop"
    deliver_partner_file(root)

    delivered = run_er(
        "ingest", "--source", PARTNER_SOURCE, "--path", str(root), "--config", str(config)
    )
    assert delivered.returncode == int(ExitCode.SUCCESS), delivered.stdout + delivered.stderr
    standardized = run_er("standardize", "--config", str(config))
    assert standardized.returncode == int(ExitCode.SUCCESS), (
        standardized.stdout + standardized.stderr
    )

    # partnerapp leaves the config: deliver a crm file and standardize with the
    # BASE document. v1 failed here at render time; the union model must not.
    (root / "crm").mkdir(parents=True)
    shutil.copy2(REPO_ROOT / "fixtures/static/base_10/base/crm.csv", root / "crm" / "crm.csv")
    delivered = run_er("ingest", "--source", "crm", "--path", str(root))
    assert delivered.returncode == int(ExitCode.SUCCESS), delivered.stdout + delivered.stderr
    standardized = run_er("standardize")
    assert standardized.returncode == int(ExitCode.SUCCESS), (
        standardized.stdout + standardized.stderr
    )

    with connect() as connection:
        counts = dict(
            query(
                connection,
                f"SELECT source_system, count(*) FROM {SCHEMA_QUALIFIER}.stg_records "
                f"GROUP BY source_system",
            )
        )
        assert counts[PARTNER_SOURCE] == len(PARTNER_ROWS), "partnerapp rows must persist inert"
        assert counts["crm"] > 0

        # The delta run left the removed source's current rows standing too.
        (partner_current,) = query(
            connection,
            f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.int_std_records WHERE source_system = ?",
            PARTNER_SOURCE,
        )[0]
        assert partner_current == len(PARTNER_ROWS)
