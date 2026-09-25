"""T-GOLD-1: survivorship values and lineage rules on `base_10` (S4.6, S5, S8.2, S8.3).

`assert_golden_equal` proves the ten assembled records equal the committed expectation
value-for-value; the lineage arm proves that *why* each value won is the rule the fixture
says, across all six S4.6 tokens (`source_priority`, `recency`, `frequency`,
`completeness`, `validated`, `tiebreak_deterministic`). Two checks exist because a weaker
version passes on a broken dispatch:

* **The deterministic tie is asserted by its winner, not just its rule.** A chain whose
  terminal element picked the physically-first row would still stamp
  `tiebreak_deterministic`; only asserting the winning `record_key` is the lexicographic
  minimum of the tied records catches that (M11).
* **The primary value assertion is proven sensitive.** Swapping `crm` and `billing`
  `priority_rank` must change at least one `source_priority`-decided value and make
  `assert_golden_equal` against the committed file FAIL — otherwise a dispatch that
  ignored `priority_rank` and took the first row would pass the primary arm vacuously.
  The flip runs from a temp config (`configs/test.yaml` is never mutated in place, V11
  keeps the ranks unique) in this function's own lake namespace.

The marts harness is duplicated from `tests/integration/test_golden_models.py` rather than
imported: a test module importing another makes a node id's dependencies invisible.

Follows the ER-092 `SPEC_TEST_IDS` convention.
"""

from __future__ import annotations

import csv
import json
import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final

import duckdb
import pandas as pd
import pytest
from helpers.compare import assert_golden_equal
from helpers.config_mutation import mutated_config
from helpers.expected import label_map_from_membership
from helpers.model import load_fixture_model
from helpers.scenario import load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode
from er.lake.columns import ADDRESS_ATTRIBUTE, ADDRESS_COMPOSITE_COLUMNS, GOLDEN_LINEAGE_ATTRIBUTES
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.full import MODE_FULL
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

#: The S8.3 rows this module realises (ER-092 convention).
SPEC_TEST_IDS: Final[tuple[str, ...]] = (
    "tests/integration/test_golden.py::test_survivorship_values_and_rules",
)

REPO_ROOT: Final = Path(__file__).resolve().parents[2]
SCENARIO: Final = "base_10"
BASE_PHASE: Final = "base"

GOLDEN_RECORDS: Final = f"{SCHEMA_QUALIFIER}.golden_records"
GOLDEN_LINEAGE: Final = f"{SCHEMA_QUALIFIER}.golden_lineage"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

MARTS_SELECTOR: Final = "marts"
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

EXPECTED_DIR: Final = REPO_ROOT / "fixtures" / "static" / SCENARIO / "expected" / BASE_PHASE
EXPECTED_GOLDEN: Final = EXPECTED_DIR / "golden.csv"
EXPECTED_LINEAGE: Final = EXPECTED_DIR / "lineage.csv"

#: The `golden_records` value columns the committed expectation compares (S8.2.1), i.e.
#: `golden.csv`'s header minus the symbolic `entity_label`.
GOLDEN_VALUE_COLUMNS: Final[tuple[str, ...]] = (
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
    "survivorship_version",
    "metadata",
)

#: S5's closed `golden_lineage.rule` vocabulary: the five chain rules plus the terminal.
RULE_VOCABULARY: Final[frozenset[str]] = frozenset(
    {
        "source_priority",
        "recency",
        "frequency",
        "completeness",
        "validated",
        "tiebreak_deterministic",
    }
)

#: The attribute each `golden_records` value column belongs to, for the flip arm. The six
#: `addr_*` columns all belong to the single `address` attribute (S4.6 composite).
_COLUMN_ATTRIBUTE: Final[dict[str, str]] = {
    column: (ADDRESS_ATTRIBUTE if column.startswith("addr_") else column)
    for column in GOLDEN_VALUE_COLUMNS
    if column not in ("survivorship_version", "metadata")
}

SOURCE_PRIORITY: Final = "source_priority"
TIEBREAK: Final = "tiebreak_deterministic"


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


@dataclass
class Marts:
    """A lake with `base_10` reconciled, ready for the marts to be built over it."""

    connection: duckdb.DuckDBPyConnection
    cfg: Config
    artifacts: Path

    def dbt(self, command: str, select: str | None = None) -> DbtResult:
        payload = render_dbt_vars(
            self.cfg, str(ULID()), extra={BLOCKING_DBT_VAR: blocking_payload(self.cfg)}
        )
        return run_dbt(
            command,
            select=select,
            vars=payload,
            target="lake",
            close_conn=lambda: detach(self.connection),
            reopen_conn=self._reattach,
            project_dir=DBT_PROJECT_DIR,
            profiles_dir=DBT_PROFILES_DIR,
            artifacts_dir=self.artifacts,
        )

    def build_marts(self) -> DbtResult:
        return self.dbt("build", select=MARTS_SELECTOR)

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
def marts(
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    object_store: ObjectStore,
    tmp_path: Path,
) -> Iterator[Marts]:
    """`base_10` ingested, standardized, scored and reconciled; marts NOT yet built."""
    connection = initialised_lake
    harness = Marts(connection=connection, cfg=cfg, artifacts=tmp_path / "artifacts")
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
            result = run_er("ingest", "--source", source, "--path", str(root), "--run-id", run_id)
            assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr
        harness.dbt("build", select=STAGING_SELECTOR)
        harness.dbt("build", select=INTERMEDIATE_SELECTOR)
        scored = run_er("match", "--mode", MODE_FULL, "--run-id", run_id, "--json")
        assert scored.returncode == int(ExitCode.SUCCESS), scored.stdout + scored.stderr
        reconciled = run_er("reconcile", "--run-id", run_id)
        assert reconciled.returncode == int(ExitCode.SUCCESS), reconciled.stdout + reconciled.stderr
        assert int(scalar(connection, f"SELECT count(*) FROM {MEMBERSHIP}")) > 0, (
            "no membership was written, so the marts would have nothing to assemble"
        )
        yield harness
    finally:
        connection.execute(f'USE "{database}".{schema}')


def _membership(connection: duckdb.DuckDBPyConnection) -> list[tuple[str, str]]:
    return [
        (str(record), str(entity))
        for record, entity in connection.execute(
            f"SELECT record_key, entity_id FROM {MEMBERSHIP}"
        ).fetchall()
    ]


def _golden_frame(connection: duckdb.DuckDBPyConnection) -> pd.DataFrame:
    columns = ("entity_id", *GOLDEN_VALUE_COLUMNS)
    rows = connection.execute(f"SELECT {', '.join(columns)} FROM {GOLDEN_RECORDS}").fetchall()
    return pd.DataFrame(rows, columns=list(columns))


def _golden_values_by_id(connection: duckdb.DuckDBPyConnection) -> dict[str, dict[str, Any]]:
    """`entity_id -> {column: value}` over the golden value columns, for diffing."""
    frame = _golden_frame(connection)
    out: dict[str, dict[str, Any]] = {}
    for record in frame.to_dict(orient="records"):
        out[str(record["entity_id"])] = {column: record[column] for column in GOLDEN_VALUE_COLUMNS}
    return out


def _expected_lineage() -> list[dict[str, str]]:
    with EXPECTED_LINEAGE.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def test_survivorship_values_and_rules(marts: Marts) -> None:
    """AC2, AC3, AC4, AC5, AC7: values equal the expectation and every rule is the one
    the fixture names, across all six tokens, with the deterministic tie asserted by its
    winner and the address composite traced to one record."""
    marts.build_marts()
    connection = marts.connection

    label_map = label_map_from_membership(_membership(connection))
    entity_of_label = dict(label_map)

    # AC2: exactly ten rows, equal to the committed expectation value-for-value.
    assert int(scalar(connection, f"SELECT count(*) FROM {GOLDEN_RECORDS}")) == 10
    assert_golden_equal(_golden_frame(connection), EXPECTED_GOLDEN, label_map)

    # AC3: the rule that decided each (entity, attribute) equals the fixture's, and all
    # six tokens are exercised.
    actual_lineage = {
        (str(entity), str(attribute)): (str(rule), str(record_key))
        for entity, attribute, rule, record_key in connection.execute(
            f"SELECT entity_id, attribute, rule, record_key FROM {GOLDEN_LINEAGE}"
        ).fetchall()
    }
    observed_tokens: set[str] = set()
    for row in _expected_lineage():
        entity_id = entity_of_label[row["entity_label"]]
        key = (entity_id, row["attribute"])
        assert key in actual_lineage, (
            f"golden_lineage has no row for {row['entity_label']}/{row['attribute']}"
        )
        actual_rule, _ = actual_lineage[key]
        assert actual_rule == row["rule"], (
            f"{row['entity_label']}/{row['attribute']}: rule is {actual_rule!r}, the fixture "
            f"says {row['rule']!r}"
        )
        observed_tokens.add(row["rule"])
    assert observed_tokens == RULE_VOCABULARY, (
        f"the asserted rows exercise {sorted(observed_tokens)}; S4.6 closes the vocabulary at "
        f"{sorted(RULE_VOCABULARY)} and T-GOLD-1 requires all six covered"
    )

    # AC4: the designed survivorship tie (base_10's W007/W008 persona) resolves by the
    # terminal rule, and its winner is the lexicographic minimum of the tied records —
    # not whichever row the scan reached first (M11). The tied set is read from the
    # partition rather than hard-coded, so the winner is computed the way the rule is.
    tie_members = sorted(
        record for record, entity in _membership(connection) if entity == entity_of_label["E10"]
    )
    assert len(tie_members) == 2, f"the tie persona E10 is not two records: {tie_members}"
    tie_winner = tie_members[0]
    tiebreak_rows = [
        (attribute, record_key)
        for (entity, attribute), (rule, record_key) in actual_lineage.items()
        if entity == entity_of_label["E10"] and rule == TIEBREAK
    ]
    assert tiebreak_rows, "the tie persona resolved no attribute by tiebreak_deterministic"
    for attribute, record_key in tiebreak_rows:
        assert record_key == tie_winner, (
            f"E10/{attribute} broke the tie to {record_key!r}; the lexicographic minimum of "
            f"the tied records {tie_members} is {tie_winner!r}"
        )

    # AC5: all six addr_* values come from the SINGLE record the address lineage names.
    # OR (not AND): the composite is violated as soon as one column disagrees.
    comparisons = " OR ".join(
        f"g.{column} IS DISTINCT FROM r.{column}" for column in ADDRESS_COMPOSITE_COLUMNS
    )
    mismatched = connection.execute(
        f"""
        SELECT g.entity_id, l.record_key
          FROM {GOLDEN_RECORDS} AS g
          JOIN {GOLDEN_LINEAGE} AS l ON l.entity_id = g.entity_id AND l.attribute = ?
          JOIN {STD_RECORDS} AS r ON r.record_key = l.record_key
         WHERE {comparisons}
        """,
        [ADDRESS_ATTRIBUTE],
    ).fetchall()
    assert not mismatched, (
        f"{len(mismatched)} entities carry an address not equal to the record their lineage "
        f"names: {mismatched[:5]}. S4.6 requires all six addr_* from one contributing record"
    )

    # AC7: one lineage row per (entity_id, attribute), attributes exactly the S4.6 set.
    duplicated = connection.execute(
        f"SELECT entity_id, attribute, count(*) FROM {GOLDEN_LINEAGE} "
        "GROUP BY entity_id, attribute HAVING count(*) > 1"
    ).fetchall()
    assert not duplicated, f"golden_lineage has duplicate (entity_id, attribute) rows: {duplicated}"
    attributes = {
        str(value)
        for (value,) in connection.execute(
            f"SELECT DISTINCT attribute FROM {GOLDEN_LINEAGE}"
        ).fetchall()
    }
    assert attributes == set(GOLDEN_LINEAGE_ATTRIBUTES), (
        f"the attribute vocabulary is {sorted(attributes)}; S4.6 closes it at "
        f"{sorted(GOLDEN_LINEAGE_ATTRIBUTES)}"
    )


def test_priority_flip_changes_source_priority_winners(marts: Marts, tmp_path: Path) -> None:
    """AC6: swapping crm/billing priority_rank changes at least one source_priority value
    and makes the committed-file comparison FAIL — proving the primary arm is sensitive."""
    connection = marts.connection
    label_map = label_map_from_membership(_membership(connection))

    # Baseline: the committed dispatch, which the primary test already pins to the file.
    marts.build_marts()
    baseline = _golden_values_by_id(connection)

    # The (entity_id, attribute) pairs the fixture says source_priority decided.
    source_priority_pairs = {
        (label_map[row["entity_label"]], row["attribute"])
        for row in _expected_lineage()
        if row["rule"] == SOURCE_PRIORITY
    }
    assert source_priority_pairs, "no source_priority rows to be sensitive to"

    # Swap the two ranks in a TEMP config (V11 keeps them unique; configs/test.yaml is
    # never touched), then rebuild the marts over the same reconciled lake.
    crm_rank = marts.cfg.sources["crm"].priority_rank
    billing_rank = marts.cfg.sources["billing"].priority_rank
    assert crm_rank != billing_rank
    flipped_path = mutated_config(
        Path(os.environ["ER_CONFIG"]),
        {"sources.crm.priority_rank": billing_rank, "sources.billing.priority_rank": crm_rank},
        dest_dir=tmp_path / "flipped_config",
    )
    flipped_cfg = load_config(flipped_path)
    Marts(
        connection=connection, cfg=flipped_cfg, artifacts=tmp_path / "artifacts_flip"
    ).build_marts()
    flipped = _golden_values_by_id(connection)

    # AC6a: at least one source_priority-decided value actually moved.
    changed = [
        (entity_id, column)
        for entity_id in baseline
        for column in GOLDEN_VALUE_COLUMNS
        if baseline[entity_id][column] != flipped[entity_id][column]
        and (entity_id, _COLUMN_ATTRIBUTE.get(column)) in source_priority_pairs
    ]
    assert changed, (
        "swapping crm/billing priority_rank moved no source_priority value; a dispatch that "
        "ignored priority_rank would pass the primary arm vacuously"
    )

    # AC6b: the committed comparison now FAILS — the primary assertion is sensitive.
    with pytest.raises(AssertionError):
        assert_golden_equal(_golden_frame(connection), EXPECTED_GOLDEN, label_map)
