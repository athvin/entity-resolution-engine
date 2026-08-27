"""The two golden marts, built against a real lake (S4.6, S5, S5.0).

B4 called `golden_records` a schema placeholder: the pipeline's terminal output had no
column list, so the mart could not be written and nothing could assert on it. These arms
are the first assertions the relation has ever had, and four of them exist because a
weaker version of the same check would pass on a broken mart:

* **The built schema is compared against S5, not against the model.** A model and a
  contract that agree with each other and disagree with the spec is exactly the drift
  `contract: {enforced: true}` exists to stop, so the expectation is read from
  `er.lake.model.REGISTRY`, which `tests/unit/test_ddl_registry.py` parses out of
  DesignDoc.md.
* **The contract is proven live by breaking it.** An `enforced: false` typo is invisible
  while the model happens to match; the only way to know the enforcement runs is to make
  a declared type disagree and watch `dbt build` refuse. The perturbation is made against
  a COPY of the project, never the committed tree.
* **The address is traced to ONE record, by join.** Comparing six values against six
  independently-derived winners would agree on any fixture whose members share an
  address. Joining `int_std_records` on the `record_key` the `address` lineage row names
  is the assertion that fails when the composite is assembled field-by-field, which is
  the defect S4.6 names.
* **`survivorship_version` is proven to come from the override.** `dbt_project.yml`
  carries a fallback so `dbt parse` renders without a config document, and a mart reading
  that fallback would be indistinguishable from one reading the config -- unless the two
  differ. So the arm rebuilds with a deliberately different value and watches the column
  follow the config.
"""

from __future__ import annotations

import json
import os
import shutil
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
    DBT_PROFILES_DIR,
    DBT_PROJECT_DIR,
    DbtResult,
    render_dbt_vars,
    run_dbt,
)
from er.errors import ExitCode, StageFailure
from er.lake.columns import (
    ADDRESS_ATTRIBUTE,
    ADDRESS_COMPOSITE_COLUMNS,
    GOLDEN_LINEAGE_ATTRIBUTES,
)
from er.lake.ducklake import attach_statements, detach
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.lake.model_registry import model_params_uri
from er.lake.objectstore import ObjectStore
from er.matching.full import MODE_FULL
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

GOLDEN_RECORDS: Final = f"{SCHEMA_QUALIFIER}.golden_records"
GOLDEN_LINEAGE: Final = f"{SCHEMA_QUALIFIER}.golden_lineage"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
MODEL_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.model_registry"

MARTS_SELECTOR: Final = "marts"
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

SCENARIO: Final = "base_10"
BASE_PHASE: Final = "base"

#: The rule vocabulary S5's `golden_lineage.rule` comment closes: the five chain rules
#: plus the terminal element's own name.
RULE_VOCABULARY: Final[frozenset[str]] = frozenset(
    {"source_priority", "recency", "frequency", "completeness", "validated"}
) | {"tiebreak_deterministic"}


def config() -> Config:
    """The validated S6 document this session runs against (S7.1)."""
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

    def dbt(
        self,
        command: str,
        select: str | None = None,
        *,
        project_dir: str = DBT_PROJECT_DIR,
        extra_vars: dict[str, object] | None = None,
    ) -> DbtResult:
        payload = render_dbt_vars(
            self.cfg, str(ULID()), extra={BLOCKING_DBT_VAR: blocking_payload(self.cfg)}
        )
        if extra_vars:
            payload.update(extra_vars)
        return run_dbt(
            command,
            select=select,
            vars=payload,
            target="lake",
            close_conn=lambda: detach(self.connection),
            reopen_conn=self._reattach,
            project_dir=project_dir,
            profiles_dir=DBT_PROFILES_DIR,
            artifacts_dir=self.artifacts,
        )

    def build_marts(self, **kwargs: Any) -> DbtResult:
        return self.dbt("build", select=MARTS_SELECTOR, **kwargs)

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
        assert int(scalar(connection, f"SELECT count(*) FROM {MEMBERSHIP}")) > 0, (
            "no membership was written, so the marts would have nothing to assemble"
        )
        yield harness
    finally:
        connection.execute(f'USE "{database}".{schema}')


def test_golden_records_schema_is_literal_s5(marts: Marts) -> None:
    """AC1: the built relation is S5's column list, in order, with S5's types."""
    marts.build_marts()

    built = [
        (str(name), str(kind))
        for name, kind in marts.connection.execute(
            "SELECT column_name, data_type FROM information_schema.columns "
            "WHERE table_name = 'golden_records' ORDER BY ordinal_position"
        ).fetchall()
    ]
    # The expectation comes from the registry, which `tests/unit/test_ddl_registry.py`
    # parses out of DesignDoc.md -- so this compares the built relation to the SPEC,
    # not to the model that produced it.
    spec = [(column.name, column.type) for column in REGISTRY["golden_records"].columns]
    assert built == spec, (
        f"golden_records was built as {built},\nS5 declares {spec}. Column order and "
        "type are both part of the contract."
    )
    assert [name for name, _ in built if name.startswith("addr_")] == list(
        ADDRESS_COMPOSITE_COLUMNS
    ), "the six addr_* columns are not expanded as S5 expands them"


def test_contract_violation_fails_dbt_build(marts: Marts, tmp_path: Path) -> None:
    """AC2: `contract: {enforced: true}` is live, proven by breaking it.

    Against a COPY of the project. A test that edited the committed `schema.yml` and
    restored it afterwards would leave the tree broken if it failed in between, and the
    gate ladder would then fail for a reason unrelated to the change.
    """
    marts.build_marts()

    project = tmp_path / "dbt_copy"
    shutil.copytree(DBT_PROJECT_DIR, project, dirs_exist_ok=False)
    schema = project / "models" / "marts" / "schema.yml"
    original = schema.read_text(encoding="utf-8")
    assert "      - name: birth_date\n        data_type: DATE" in original, (
        "the perturbation target moved; this arm would silently stop proving anything"
    )
    schema.write_text(
        original.replace(
            "      - name: birth_date\n        data_type: DATE",
            "      - name: birth_date\n        data_type: VARCHAR",
        ),
        encoding="utf-8",
    )

    with pytest.raises(StageFailure) as raised:
        marts.build_marts(project_dir=str(project))
    assert "dbt build" in str(raised.value), (
        f"the build failed for an unexpected reason: {raised.value}"
    )


def test_lineage_grid_is_complete_and_vocabulary_closed(marts: Marts) -> None:
    """AC3: six rows per entity, and both vocabularies closed."""
    marts.build_marts()

    entities = int(
        scalar(marts.connection, f"SELECT count(DISTINCT entity_id) FROM {GOLDEN_RECORDS}")
    )
    assert entities > 0, "no golden rows were built"

    lineage = int(scalar(marts.connection, f"SELECT count(*) FROM {GOLDEN_LINEAGE}"))
    assert lineage == len(GOLDEN_LINEAGE_ATTRIBUTES) * entities, (
        f"{lineage} lineage rows for {entities} entities; the grid is complete, so it is "
        f"{len(GOLDEN_LINEAGE_ATTRIBUTES)} per entity even where the winning value is NULL"
    )

    attributes = {
        str(value)
        for (value,) in marts.connection.execute(
            f"SELECT DISTINCT attribute FROM {GOLDEN_LINEAGE}"
        ).fetchall()
    }
    assert attributes == set(GOLDEN_LINEAGE_ATTRIBUTES), (
        f"the attribute vocabulary is {sorted(attributes)}; S4.6 closes it at "
        f"{sorted(GOLDEN_LINEAGE_ATTRIBUTES)}"
    )

    rules = {
        str(value)
        for (value,) in marts.connection.execute(
            f"SELECT DISTINCT rule FROM {GOLDEN_LINEAGE}"
        ).fetchall()
    }
    assert rules <= RULE_VOCABULARY, (
        f"{sorted(rules - RULE_VOCABULARY)} are outside S5's closed rule vocabulary"
    )

    # Every entity carries every token exactly once: the logical key, checked as data
    # rather than only as a dbt test, so a failure names the entity.
    holes = marts.connection.execute(
        f"SELECT entity_id, count(*) FROM {GOLDEN_LINEAGE} GROUP BY entity_id "
        f"HAVING count(*) <> {len(GOLDEN_LINEAGE_ATTRIBUTES)}"
    ).fetchall()
    assert not holes, f"entities with an incomplete lineage grid: {holes[:5]}"


def test_address_columns_come_from_one_record(marts: Marts) -> None:
    """AC4: all six addr_* values trace to the SINGLE record the lineage row names.

    S4.6's one rule this stage owns. Joining `int_std_records` on that record_key is what
    catches a field-by-field assembly; comparing six separately-derived winners would
    agree with a broken mart on any fixture whose members share an address.
    """
    marts.build_marts()

    # OR, not AND: the rule is violated as soon as ONE of the six disagrees with the
    # record the lineage names, and an AND would only catch an address that differed in
    # every column at once -- which a field-by-field assembly never produces.
    comparisons = " OR ".join(
        f"g.{column} IS DISTINCT FROM r.{column}" for column in ADDRESS_COMPOSITE_COLUMNS
    )
    mismatched = marts.connection.execute(
        f"""
        SELECT g.entity_id, l.record_key
          FROM {GOLDEN_RECORDS} AS g
          JOIN {GOLDEN_LINEAGE} AS l
            ON l.entity_id = g.entity_id AND l.attribute = ?
          JOIN {STD_RECORDS} AS r
            ON r.record_key = l.record_key
         WHERE {comparisons}
        """,
        [ADDRESS_ATTRIBUTE],
    ).fetchall()
    assert not mismatched, (
        f"{len(mismatched)} entities carry an address that does not match the record "
        f"their lineage names: {mismatched[:5]}. S4.6 requires all six columns to come "
        "from the single winning contributing record."
    )

    # And the arm is not vacuous: every entity must actually have an address lineage row.
    assert int(
        scalar(
            marts.connection,
            f"SELECT count(*) FROM {GOLDEN_LINEAGE} WHERE attribute = ?",
            ADDRESS_ATTRIBUTE,
        )
    ) == int(scalar(marts.connection, f"SELECT count(*) FROM {GOLDEN_RECORDS}"))


def test_non_address_values_come_from_their_lineage_record(marts: Marts) -> None:
    """AC5: each non-address value equals the value on the record its lineage names."""
    marts.build_marts()

    scalar_attributes = [a for a in GOLDEN_LINEAGE_ATTRIBUTES if a != ADDRESS_ATTRIBUTE]
    assert scalar_attributes, "the vocabulary collapsed to the address alone"

    for attribute in scalar_attributes:
        mismatched = marts.connection.execute(
            f"""
            SELECT g.entity_id, l.record_key, g.{attribute}, r.{attribute}
              FROM {GOLDEN_RECORDS} AS g
              JOIN {GOLDEN_LINEAGE} AS l
                ON l.entity_id = g.entity_id AND l.attribute = ?
              JOIN {STD_RECORDS} AS r
                ON r.record_key = l.record_key
             WHERE g.{attribute} IS DISTINCT FROM r.{attribute}
            """,
            [attribute],
        ).fetchall()
        assert not mismatched, (
            f"{attribute}: {len(mismatched)} entities carry a value their lineage record "
            f"does not hold: {mismatched[:5]}"
        )


def test_survivorship_version_comes_from_config_vars(marts: Marts) -> None:
    """AC6: the column follows the S6 document, not the dbt_project.yml fallback."""
    marts.build_marts()

    from_config = marts.cfg.versions.survivorship_version
    built = {
        str(value)
        for (value,) in marts.connection.execute(
            f"SELECT DISTINCT survivorship_version FROM {GOLDEN_RECORDS}"
        ).fetchall()
    }
    assert built == {from_config}, (
        f"golden_records carries survivorship_version {built}, the config says {from_config!r}"
    )

    # Rebuild with a different value and watch the column follow it. Without this, a
    # mart reading the `dbt_project.yml` fallback is indistinguishable from one reading
    # the config whenever the two happen to agree -- which today they do.
    overridden = f"{from_config}-probe"
    assert overridden != from_config
    marts.build_marts(extra_vars={"survivorship_version": overridden})

    rebuilt = {
        str(value)
        for (value,) in marts.connection.execute(
            f"SELECT DISTINCT survivorship_version FROM {GOLDEN_RECORDS}"
        ).fetchall()
    }
    assert rebuilt == {overridden}, (
        f"changing the var left survivorship_version at {rebuilt}; the mart is reading "
        "the dbt_project.yml fallback rather than the override"
    )
    lineage = {
        str(value)
        for (value,) in marts.connection.execute(
            f"SELECT DISTINCT survivorship_version FROM {GOLDEN_LINEAGE}"
        ).fetchall()
    }
    assert lineage == {overridden}, "golden_lineage did not follow the same override"


def test_every_active_entity_with_members_has_exactly_one_golden_row(marts: Marts) -> None:
    """AC7, as data: the dbt tests assert it too, but a failure here names the entity."""
    marts.build_marts()

    missing = marts.connection.execute(
        f"""
        SELECT m.entity_id, count(*) AS members
          FROM {MEMBERSHIP} AS m
          JOIN {SCHEMA_QUALIFIER}.entities AS e
            ON e.entity_id = m.entity_id AND e.status = 'active'
         WHERE m.entity_id NOT IN (SELECT entity_id FROM {GOLDEN_RECORDS})
         GROUP BY m.entity_id
        """
    ).fetchall()
    assert not missing, (
        f"{len(missing)} active entities hold members but have no golden row: {missing[:5]}"
    )

    duplicated = marts.connection.execute(
        f"SELECT entity_id, count(*) FROM {GOLDEN_RECORDS} GROUP BY entity_id HAVING count(*) > 1"
    ).fetchall()
    assert not duplicated, f"entities with more than one golden row: {duplicated[:5]}"
