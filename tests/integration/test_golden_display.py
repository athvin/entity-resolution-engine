"""`golden_display` built against a real lake (S4.6, S5).

Three claims, and the third is the one the relation exists to make safe:

* **The schema is S5's and there is one row per entity.** Compared against
  `er.lake.model.REGISTRY`, which `tests/unit/test_ddl_registry.py` parses out of
  DesignDoc.md — so this compares the built relation to the SPEC rather than to the model
  that produced it.
* **The four transforms are pinned**, each with a positive and a NULL-handling case. They
  are asserted against rows CONSTRUCTED for the purpose rather than against whatever
  `base_10` happens to contain: a transform test that only sees well-populated rows never
  exercises the branch where a part is missing, and that branch is where a leading space
  or a dangling comma appears.
* **A display rebuild leaves the matching layer byte-identical.** S4.6's prohibition,
  executed. Row counts would pass on a relation re-cased in place, so the arm digests the
  CONTENT of `int_std_records`, `int_blocking_keys` and `match_scores` — order-independent,
  because DuckLake promises no physical order and a false failure there would look
  exactly like a real one.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import load_fixture_model
from helpers.scenario import load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import (
    ARTIFACTS_DIR,
    DBT_PROFILES_DIR,
    DBT_PROJECT_DIR,
    DbtResult,
    render_dbt_vars,
    run_dbt,
)
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.full import MODE_FULL
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

GOLDEN_RECORDS: Final = f"{SCHEMA_QUALIFIER}.golden_records"
GOLDEN_DISPLAY: Final = f"{SCHEMA_QUALIFIER}.golden_display"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

#: The relations a display rebuild must not touch. All three are matching-layer inputs
#: or outputs; re-casing any of them is what S4.6's prohibition forbids.
MATCHING_RELATIONS: Final[tuple[str, ...]] = (
    "int_std_records",
    "int_blocking_keys",
    "match_scores",
)

MARTS_SELECTOR: Final = "marts"
DISPLAY_SELECTOR: Final = "golden_display"
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

SCENARIO: Final = "base_10"
BASE_PHASE: Final = "base"


def config() -> Config:
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    payload, _ = blocking_rules_from_config(cfg)
    return payload


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def content_digest(connection: duckdb.DuckDBPyConnection, relation: str) -> str:
    """An order-independent digest of a relation's whole content.

    Order-independent on purpose: DuckLake promises no physical row order, so a digest
    that depended on it would fail after an unrelated compaction and the failure would be
    indistinguishable from the re-casing this test exists to catch.
    """
    return str(
        scalar(
            connection,
            f"SELECT coalesce(md5(string_agg(_row, '|' ORDER BY _row)), 'empty') "
            f"FROM (SELECT to_json({SCHEMA_QUALIFIER}.{relation})::VARCHAR AS _row "
            f"FROM {SCHEMA_QUALIFIER}.{relation}) AS _t",
        )
    )


@dataclass
class Display:
    connection: duckdb.DuckDBPyConnection
    cfg: Config
    artifacts: Path

    def dbt(self, command: str, select: str | None = None) -> DbtResult:
        return run_dbt(
            command,
            select=select,
            vars=render_dbt_vars(
                self.cfg, str(ULID()), extra={BLOCKING_DBT_VAR: blocking_payload(self.cfg)}
            ),
            target="lake",
            close_conn=lambda: detach(self.connection),
            reopen_conn=self._reattach,
            project_dir=DBT_PROJECT_DIR,
            profiles_dir=DBT_PROFILES_DIR,
            artifacts_dir=ARTIFACTS_DIR,
        )

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


@pytest.fixture(scope="session")
def cfg() -> Config:
    return config()


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    if (Path(DBT_PROJECT_DIR) / "dbt_packages" / "dbt_utils").is_dir():
        return
    completed = subprocess.run(
        ["dbt", "deps", "--project-dir", DBT_PROJECT_DIR, "--profiles-dir", DBT_PROFILES_DIR],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


@pytest.fixture
def display(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Display]:
    """`base_10` reconciled and all three marts built."""
    connection = initialised_lake
    harness = Display(connection=connection, cfg=cfg, artifacts=ARTIFACTS_DIR)
    harness.dbt("seed")

    database = str(scalar(connection, "SELECT current_database()"))
    schema = str(scalar(connection, "SELECT current_schema()"))
    try:
        model_version, _, settings = load_fixture_model(connection)
        published = model_params_uri(cfg.storage.model_uri_prefix, model_version)
        object_store.put_bytes(published, json.dumps(settings).encode("utf-8"))
        connection.execute(
            f"UPDATE {MODEL_REGISTRY} SET params_path = ? WHERE model_version = ?",
            [published, model_version],
        )

        scenario = load_scenario(SCENARIO)
        run_id = str(ULID())
        root = tmp_path / "drop"
        for source, path in scenario.inputs_for(BASE_PHASE).items():
            directory = root / source
            directory.mkdir(parents=True, exist_ok=True)
            (directory / path.name).write_bytes(path.read_bytes())
        for source in scenario.inputs_for(BASE_PHASE):
            result = run_er(
                "ingest",
                "--source",
                source,
                "--path",
                str(root),
                "--run-id",
                run_id,
                "--json",
            )
            assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
        harness.dbt("build", select=STAGING_SELECTOR)
        harness.dbt("build", select=INTERMEDIATE_SELECTOR)
        scored = run_er("match", "--mode", MODE_FULL, "--run-id", run_id, "--json")
        assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr
        reconciled = run_er("reconcile", "--run-id", run_id)
        assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr
        harness.dbt("build", select=MARTS_SELECTOR)
        yield harness
    finally:
        connection.execute(f'USE "{database}".{schema}')


def test_display_schema_and_one_row_per_entity(display: Display) -> None:
    """AC1, AC2: S5's five columns, and exactly one row per golden entity."""
    built = [
        (str(name), str(kind))
        for name, kind in display.connection.execute(
            "SELECT column_name, data_type FROM information_schema.columns "
            "WHERE table_name = 'golden_display' ORDER BY ordinal_position"
        ).fetchall()
    ]
    spec = [(column.name, column.type) for column in REGISTRY["golden_display"].columns]
    assert built == spec, f"golden_display was built as {built},\nS5 declares {spec}"

    assert "survivorship_version" not in {name for name, _ in built}, (
        "golden_display grew a survivorship_version. S4.6 makes its provenance a join to "
        "golden_records/golden_lineage; a column here is a second answer that goes stale."
    )

    records = int(scalar(display.connection, f"SELECT count(*) FROM {GOLDEN_RECORDS}"))
    shown = int(scalar(display.connection, f"SELECT count(*) FROM {GOLDEN_DISPLAY}"))
    assert records > 0, "no golden rows were built, so the comparison would be vacuous"
    assert shown == records, f"{shown} display rows for {records} golden rows"

    orphans = display.connection.execute(
        f"SELECT entity_id FROM {GOLDEN_DISPLAY} "
        f"WHERE entity_id NOT IN (SELECT entity_id FROM {GOLDEN_RECORDS})"
    ).fetchall()
    assert not orphans, f"display rows for entities with no golden row: {orphans[:5]}"


def test_display_transforms_are_pinned(display: Display) -> None:
    """AC3, AC4: the four transforms, each with a populated and a NULL-part case.

    Asserted against rows constructed here rather than against whatever `base_10`
    contains. The branch that matters is the one where a part is missing — that is where
    a leading space or a dangling comma appears — and a fixture full of well-populated
    records never reaches it.
    """
    connection = display.connection
    probe = f"{SCHEMA_QUALIFIER}.golden_records"

    stamp = scalar(connection, f"SELECT max(assembled_at) FROM {probe}")
    connection.execute(
        f"""
        INSERT INTO {probe} VALUES
          ('01M0DISPLAYPROBE000000001', 'robert', 'chen', 'Robert.Chen@Example.COM',
           '+14155550132', '742', 'Evergreen Terrace', 'Apt 3', 'Springfield', 'OR',
           '97477', DATE '1980-01-15', '1', ?),
          ('01M0DISPLAYPROBE000000002', NULL, 'okonkwo', NULL,
           '+442071838750', NULL, NULL, NULL, 'London', NULL, NULL,
           NULL, '1', ?)
        """,
        [stamp, stamp],
    )
    display.dbt("build", select=DISPLAY_SELECTOR)

    full = connection.execute(
        f"SELECT display_name, display_email, display_phone, display_address "
        f"FROM {GOLDEN_DISPLAY} WHERE entity_id = ?",
        ["01M0DISPLAYPROBE000000001"],
    ).fetchone()
    assert full is not None, "the populated probe row produced no display row"
    name, email, phone, address = (None if v is None else str(v) for v in full)

    assert name == "Robert Chen", f"display_name is {name!r}"
    # AC4: byte-equal, so the display layer never re-cases an email.
    assert email == "Robert.Chen@Example.COM", f"display_email is {email!r}, not verbatim"
    assert phone == "(415) 555-0132", f"display_phone is {phone!r}"
    assert address == "742 Evergreen Terrace Apt 3, Springfield OR 97477", (
        f"display_address is {address!r}"
    )
    for rendered in (name, address):
        assert rendered is not None
        assert "  " not in rendered and not rendered.startswith(" "), (
            f"{rendered!r} carries an empty separator run"
        )

    sparse = connection.execute(
        f"SELECT display_name, display_email, display_phone, display_address "
        f"FROM {GOLDEN_DISPLAY} WHERE entity_id = ?",
        ["01M0DISPLAYPROBE000000002"],
    ).fetchone()
    assert sparse is not None, "the sparse probe row produced no display row"
    s_name, s_email, s_phone, s_address = (None if v is None else str(v) for v in sparse)

    assert s_name == "Okonkwo", (
        f"a NULL given_name yielded {s_name!r}; the missing part must take its space"
    )
    assert s_email is None, f"a NULL email rendered as {s_email!r}"
    # Not a +1 NANP number, so it passes through rather than being forced into a US shape.
    assert s_phone == "+442071838750", f"an international number was reformatted: {s_phone!r}"
    assert s_address == "London", (
        f"display_address is {s_address!r}; empty parts and their separators are dropped"
    )


def test_matching_inputs_unchanged_by_display_rebuild(display: Display) -> None:
    """AC5: S4.6's prohibition, executed — rebuilding the display re-cases nothing."""
    before = {
        relation: content_digest(display.connection, relation) for relation in MATCHING_RELATIONS
    }
    assert all(digest != "empty" for digest in before.values()), (
        f"a matching relation is empty before the rebuild: {before}. The comparison would "
        "hold trivially."
    )

    display.dbt("build", select=DISPLAY_SELECTOR)

    after = {
        relation: content_digest(display.connection, relation) for relation in MATCHING_RELATIONS
    }
    changed = [relation for relation in MATCHING_RELATIONS if before[relation] != after[relation]]
    assert not changed, (
        f"{changed} changed when golden_display was rebuilt. S4.6 makes the display "
        "presentation casing only, so matching-layer data must be untouched by it."
    )
