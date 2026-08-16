"""`base_10` blocked into `int_blocking_keys` (S4.2, S5, S5.0, S6, S8.1).

`int_blocking_keys` is not evidence -- S4.3.4 forbids it as a scoring input, and
Splink regenerates its own candidate pairs from the same generator. What it IS is the
executable form of `configs/*.yaml` `blocking:`: the rule source, the S4.5
touched-subgraph driver and the `candidate_pair_count` benchmark metric. So the
claims worth asserting are that the config decides the key set completely, that the
S4.2 NULL/empty policy holds on every row, and that the relation's rows behave the
way a `delete+insert` on a many-rows-per-record relation has to.

Seven properties, one test each:

* **the key set is the config's** -- the distinct `key_type` set equals the config's,
  and the relation builds green under its enforced S5 contract.
* **the NULL/empty policy** -- no row carries a NULL or empty `key_value`. Both halves
  matter: `is not null` alone would let the empty string block every record whose key
  expression concatenates a missing component, and Splink's `block_on` never joins on
  NULL (S4.2).
* **missing and placeholder emails** -- the four records delivered with an empty email
  and the two carrying a `standardization.email_placeholders` address emit no
  `email_exact` row at all (S8.2's `missing_emails` and `placeholder_email` traps).
* **the drifted phones** -- three spellings of one number normalize to one E.164 value,
  so the trap's three records share one `phone_exact` key and block together.
* **macro generation** -- the compiled SQL is one S4.2 branch per payload entry and
  nothing else, and a fifth `blocking:` entry on a COPIED config adds exactly one
  branch and one key type with no edit to the model file. This is the property that
  removes the drift M12 identified, and it is the only one that cannot be asserted
  against rows.
* **delete+insert** -- when a record's standardized values change, every one of that
  record's prior rows goes and its new full key set lands, with no other record's rows
  disturbed (M16).
* **the logical key and the domain** -- DuckLake enforces neither, so a duplicate
  `(key_type, key_value, record_key)` and an unconfigured `key_type` are caught by dbt
  tests or they are not caught (S5.0).

Two deliberate choices about how the models are driven, both inherited from
`tests/integration/scenarios/test_int_std_records.py` and made for the same reasons.

dbt is invoked through `er.dbt_runner.run_dbt` with the `--vars` payload
`render_dbt_vars` builds, plus the S4.2 blocking payload under
`er.matching.model.BLOCKING_DBT_VAR` -- which is what `er standardize` will pass once
it is wired, and is not a second rendering of the config: the payload comes from
`blocking_rules_from_config`, the one generator S4.2 names. `er standardize` itself is
still ER-014's `NoOpStage` and returns `10` without touching a model, so driving it
would assert nothing about this relation.

The `Dbt` harness and the small query helpers are duplicated from that module rather
than imported. A test module importing another test module makes a node id's
dependencies invisible to whoever reads it, and `tests/integration/test_keys.py`
records the same reasoning.
"""

from __future__ import annotations

import csv
import json
import os
import re
import subprocess
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

import duckdb
import pytest
import yaml
from helpers.scenario import Scenario, load_scenario
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
from er.lake.dbt_sources import KEYS_TAG
from er.lake.ddl import live_columns
from er.lake.ducklake import attach_statements, connect, detach
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

#: The S8.2 fixture this suite blocks, and its only phase.
SCENARIO_NAME = "base_10"
PHASE = "base"

#: The three sources `configs/test.yaml` declares, in `sources:` order.
SOURCES: tuple[str, ...] = ("crm", "billing", "webforms")

#: The relation under test and the two selectors `er standardize` will run.
MODEL = "int_blocking_keys"
INTERMEDIATE_SELECTOR = "intermediate"
STAGING_SELECTOR = "staging"

#: `base_10` is 23 records over 10 personas (S8.2), every one of them current.
TOTAL_ROWS = 23

#: AC1's literal, and deliberately a literal: it is `configs/test.yaml`'s `blocking:`
#: block transcribed, so a test that only compared the relation against the config
#: would pass on a config that had lost an entry.
EXPECTED_KEY_TYPES = frozenset({"email_exact", "phone_exact", "name_postal", "dob_name"})

#: T-KEY-1b's selector, from the tag ER-021 puts on every logical-key test.
KEYS_SELECTOR = f"tag:{KEYS_TAG}"

#: The `accepted_values` node `schema.yml` names, so a failure can be attributed to
#: the domain check rather than to dbt having objected to something.
DOMAIN_TEST = "accepted_values_int_blocking_keys_key_type"

#: The S8.2 traps this suite reads, and what each one is worth.
DRIFTED_PHONES_TRAP = "drifted_phones"
MISSING_EMAILS_TRAP = "missing_emails"
PLACEHOLDER_EMAIL_TRAP = "placeholder_email"
TRAPS_FILE = "traps.csv"

#: The one E.164 number `(415) 555-0132`, `415-555-0132` and `+14155550132` all
#: normalize to (S4.2 `phone_e164`), which is what makes the trap's three records
#: block together on one key.
DRIFTED_PHONE_E164 = "+14155550132"

#: AC5's fifth entry. `addr_postal` is a column of `int_std_records` (S5), so S6.1 V6
#: accepts it, and no existing entry uses it alone -- a fifth branch that duplicated an
#: existing key type would be rejected by V7 before it could prove anything.
EXTRA_KEY_TYPE = "postal_only"
EXTRA_EXPR = "addr_postal"

#: AC7's unconfigured value. Nothing in S6 emits it, which is the whole point.
UNCONFIGURED_KEY_TYPE = "not_a_configured_key_type"

#: AC6's replacement address and the version that carries it. The instant is later
#: than anything `er ingest` stamped in this session, because "current" is defined by
#: `ingested_at` and a version that is not the greatest changes nothing (S4.2).
CHANGED_EMAIL = "changed.address@example.com"
CHANGED_INSTANT = datetime(2030, 1, 1)
CHANGED_HASH = "e" * 64

#: The model source and its compiled artifact. The Jinja header is stripped before the
#: source is searched for key logic: a comment naming a column is documentation, and
#: what AC5 forbids is an expression the model would execute.
MODEL_SQL = Path(DBT_PROJECT_DIR) / "models" / "intermediate" / f"{MODEL}.sql"
COMPILED_ROOT = Path(DBT_PROJECT_DIR) / "target" / "compiled" / "er" / "models" / "intermediate"
COMPILED_MODEL = COMPILED_ROOT / f"{MODEL}.sql"

#: dbt's resolved node config, which is where the incremental configuration actually
#: lives: `{{ config() }}` is stripped from the compiled SQL, and the manifest records
#: what dbt resolved from the model AND from `dbt_project.yml` together.
MANIFEST = Path(DBT_PROJECT_DIR) / "target" / "manifest.json"
MODEL_NODE = f"model.er.{MODEL}"

#: S4.2's incremental configuration for the `int_*` family, as dbt resolves it. The
#: `unique_key` is the RECORD, not the logical key: `delete+insert` has to remove every
#: row a touched record owns before re-inserting its full key set (M16).
EXPECTED_MODEL_CONFIG: Mapping[str, Any] = {
    "materialized": "incremental",
    "incremental_strategy": "delete+insert",
    "unique_key": ["source_system", "source_record_id"],
    "on_schema_change": "append_new_columns",
}

#: S4.2's branch template, transcribed. `<relation>` stands where the model writes
#: `{{ ref('int_std_records') }}`, because what the compiler emits is the rendered
#: relation and the template is compared against the compiled artifact.
BRANCH_TEMPLATE = (
    "select '<key_type>' as key_type, <expr> as key_value,\n"
    "       record_key, source_system, source_record_id\n"
    "from <relation>\n"
    "where <expr> is not null and <expr> <> ''"
)

#: What separates two branches in the compiled SQL.
BRANCH_SEPARATOR = "\n\nunion all\n\n"

_JINJA_HEADER = re.compile(r"^\{#.*?#\}", re.DOTALL)
_FROM_LINE = re.compile(r"^from (?P<relation>.+)$", re.MULTILINE)


def config() -> Config:
    """The validated S6 document this session runs against."""
    return load_config(Path(os.environ["ER_CONFIG"]))


def blocking_payload(cfg: Config) -> BlockingPayload:
    """The dbt var payload for one config, from S4.2's one generator."""
    payload, _ = blocking_rules_from_config(cfg)
    return payload


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def query(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: Any) -> list[Any]:
    return connection.execute(sql, list(parameters)).fetchall()


def row_count(connection: duckdb.DuckDBPyConnection, relation: str) -> int:
    (count,) = query(connection, f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{relation}")[0]
    return int(count)


def key_rows(connection: duckdb.DuckDBPyConnection) -> set[tuple[Any, ...]]:
    """Every row of `int_blocking_keys`, as a set.

    A set and not a list: the relation declares no order, and what S4.2 fixes about a
    re-run is which keys exist, not which order the union happened to emit them in.
    """
    return set(
        query(
            connection,
            f"SELECT key_type, key_value, record_key, source_system, source_record_id "
            f"FROM {SCHEMA_QUALIFIER}.{MODEL}",
        )
    )


def key_types(connection: duckdb.DuckDBPyConnection) -> set[str]:
    return {
        str(row[0])
        for row in query(connection, f"SELECT DISTINCT key_type FROM {SCHEMA_QUALIFIER}.{MODEL}")
    }


def deliver(scenario: Scenario, root: Path) -> Path:
    """Materialise the scenario's phase as the drop-folder root `er ingest --path` reads.

    A scenario stores a phase as `<phase>/<source>.csv` (S8.2.1) while S4.1 reads
    `<path>/<source>/`, so the files are copied rather than pointed at.
    """
    for source, path in scenario.inputs_for(PHASE).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def delivered_rows(scenario: Scenario, source: str) -> list[dict[str, str]]:
    """One source CSV, as the dicts the adapter would read (S4.1)."""
    with scenario.inputs_for(PHASE)[source].open(encoding="utf-8", newline="") as handle:
        return list(csv.DictReader(handle))


def trap_record_keys(scenario: Scenario, trap: str) -> set[str]:
    """The `record_key`s one S8.2.1 trap indexes.

    Read from `traps.csv` rather than transcribed: the trap file is the fixture's own
    statement of which records carry which designed defect, and a literal list here
    would keep passing after the fixture moved one.
    """
    with scenario.truth[TRAPS_FILE].open(encoding="utf-8", newline="") as handle:
        return {
            f"{row['source_system']}:{row['source_record_id']}"
            for row in csv.DictReader(handle)
            if row["trap"] == trap
        }


def compiled_branches() -> list[str]:
    """The compiled model, split into its `UNION ALL` branches."""
    return COMPILED_MODEL.read_text(encoding="utf-8").strip().split(BRANCH_SEPARATOR)


def branch_relation(branch: str) -> str:
    """The relation one compiled branch selects from."""
    match = _FROM_LINE.search(branch)
    assert match is not None, branch
    return match.group("relation")


def expected_branch(entry: Mapping[str, str], relation: str) -> str:
    """S4.2's template with this payload entry's `key_type` and `expr` substituted."""
    return (
        BRANCH_TEMPLATE.replace("<relation>", relation)
        .replace("<key_type>", entry["key_type"])
        .replace("<expr>", entry["expr"])
    )


def model_body() -> str:
    """`int_blocking_keys.sql` with its Jinja header comment removed."""
    return _JINJA_HEADER.sub("", MODEL_SQL.read_text(encoding="utf-8"), count=1)


def config_with_extra_rule(destination: Path) -> Config:
    """A COPY of this session's config carrying one more `blocking:` entry.

    A copied document and not a mutated `Config`: what AC5 claims is that adding an
    entry to the YAML changes the compiled SQL, and going through `load_config` is what
    puts S6.1's validators between the edit and the payload.
    """
    document = yaml.safe_load(Path(os.environ["ER_CONFIG"]).read_text(encoding="utf-8"))
    document["blocking"].append({"key_type": EXTRA_KEY_TYPE, "expr": EXTRA_EXPR})
    destination.write_text(yaml.safe_dump(document, sort_keys=False), encoding="utf-8")
    return load_config(destination)


def append_changed_version(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    source_system: str,
    source_record_id: str,
) -> None:
    """Append a `raw_records` version of one key whose email differs (S4.1: append-only).

    The patch names the SOURCE column from S6's `sources.<name>.columns` mapping rather
    than a literal, because what has to change is the field the standardization macros
    read; patching some other key would leave the standardized row identical and the
    test would assert nothing.
    """
    patch = json.dumps({cfg.sources[source_system].columns["email"]: CHANGED_EMAIL})
    connection.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.raw_records "
        f"(source_system, source_record_id, payload, content_hash, "
        f" is_deleted, deleted_at, ingest_batch_id, ingested_at) "
        f"SELECT source_system, source_record_id, "
        f"       json_merge_patch(payload, CAST(? AS JSON)), ?, FALSE, NULL, ?, ? "
        f"FROM {SCHEMA_QUALIFIER}.raw_records "
        f"WHERE source_system = ? AND source_record_id = ? "
        f"ORDER BY ingested_at DESC LIMIT 1",
        [
            patch,
            CHANGED_HASH,
            str(ULID()),
            CHANGED_INSTANT,
            source_system,
            source_record_id,
        ],
    )


@dataclass
class Dbt:
    """dbt as a stage invokes it: real `--vars`, no connection spanning it (S4.0b).

    `cfg` is an attribute and not a call each time, so AC5's copied config can be swapped
    in and every later invocation renders from it -- which is what "recompiling" means
    when the only thing that changed is the document.
    """

    connection: duckdb.DuckDBPyConnection
    artifacts: Path
    cfg: Config

    def __call__(
        self,
        command: str,
        select: str | None = None,
        *,
        project_dir: str = DBT_PROJECT_DIR,
    ) -> DbtResult:
        return run_dbt(
            command,
            select=select,
            vars=render_dbt_vars(
                self.cfg, str(ULID()), extra={BLOCKING_DBT_VAR: blocking_payload(self.cfg)}
            ),
            target="lake",
            close_conn=lambda: detach(self.connection),
            reopen_conn=self._reattach,
            project_dir=project_dir,
            profiles_dir=str(Path(project_dir) / "profiles"),
            artifacts_dir=self.artifacts,
        )

    def standardize(self) -> None:
        """The selection `er standardize` runs: staging, then intermediate (S4.2)."""
        self("build", select=STAGING_SELECTOR)
        self("build", select=INTERMEDIATE_SELECTOR)

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)

    def last_output(self) -> str:
        """The captured stdout+stderr of the most recent invocation.

        `run_dbt` persists it on the failure path too, which is what lets a negative arm
        assert on WHAT dbt objected to rather than merely that it objected.
        """
        logs = sorted((self.artifacts / "dbt").glob("*.log"), key=lambda path: path.stat().st_mtime)
        assert logs, "run_dbt writes a log for every invocation"
        return logs[-1].read_text(encoding="utf-8")


@pytest.fixture
def base_10() -> Scenario:
    """The S8.2 fixture, opened through ER-028's loader."""
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (PHASE,)
    return scenario


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored.

    The image ships no `dbt/dbt_packages` (`.dockerignore`), and this relation's logical
    key is a `dbt_utils.unique_combination_of_columns`. Run as a plain subprocess: `deps`
    touches no warehouse, so it is the one invocation that is not a stage.
    """
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
def dbt(dbt_packages: None, initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path) -> Dbt:
    """dbt bound to this test's lake handle, its own artifacts root and this session's config."""
    return Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=config())


@pytest.fixture
def standardized(base_10: Scenario, dbt: Dbt, tmp_path: Path) -> Dbt:
    """`base_10` ingested and standardized through staging and intermediate.

    The seed runs first and is not part of either selection: `name_variants` reaches
    `nickname_variants` with `ref()` (S4.2), and neither path selector names it.
    """
    delivery = deliver(base_10, tmp_path / "drop")
    for source in base_10.inputs_for(PHASE):
        result = run_er("ingest", "--source", source, "--path", str(delivery))
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    dbt("seed")
    dbt.standardize()
    return dbt


def test_key_type_set_equals_config(standardized: Dbt) -> None:
    """AC1: the emitted key types are exactly the configured ones, under a green contract."""
    configured = {entry["key_type"] for entry in blocking_payload(config())}
    assert configured == set(EXPECTED_KEY_TYPES), configured

    with connect() as connection:
        emitted = key_types(connection)
        blocking_rows = row_count(connection, MODEL)
        by_type = {
            str(key): int(count)
            for key, count in query(
                connection,
                f"SELECT key_type, count(*) FROM {SCHEMA_QUALIFIER}.{MODEL} GROUP BY key_type",
            )
        }
        shape = dict(live_columns(connection, MODEL))

    assert emitted == set(EXPECTED_KEY_TYPES)
    # The two S4.2 standardize counters this relation feeds. Asserted as counters and
    # not merely as a row count, because `blocking_keys_by_type` is what tells an
    # operator that a configured rule produced nothing -- a total alone cannot.
    assert set(by_type) == set(EXPECTED_KEY_TYPES)
    assert sum(by_type.values()) == blocking_rows
    assert all(count > 0 for count in by_type.values()), by_type

    # The S5 column list, order and types, against the registry rather than against a
    # list retyped here: S5 is the review-time authority for a dbt-owned relation and
    # the contract is what makes it one (S5.0).
    assert list(shape.items()) == [(column.name, column.type) for column in REGISTRY[MODEL].columns]

    # AC1's second clause. `--select int_blocking_keys` builds the model alone, under
    # the enforced contract `dbt_project.yml` puts on every dbt-owned relation.
    standardized("build", select=MODEL)

    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    resolved = manifest["nodes"][MODEL_NODE]["config"]
    assert resolved["contract"]["enforced"] is True
    for key, value in EXPECTED_MODEL_CONFIG.items():
        assert resolved[key] == value, f"{key}: {resolved[key]!r} != {value!r}"


def test_no_null_or_empty_key_values(standardized: Dbt) -> None:
    """AC2: S4.2's NULL/empty policy, on every row of the relation."""
    with connect() as connection:
        (offending,) = query(
            connection,
            f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{MODEL} "
            f"WHERE key_value is null OR key_value = ''",
        )[0]
        total = row_count(connection, MODEL)

    assert offending == 0
    # Non-vacuous: an empty relation satisfies the count above trivially, and two of
    # the four configured expressions concatenate columns that ARE missing on some
    # `base_10` records -- which is exactly what the `<> ''` half of the predicate is
    # there for.
    assert total > 0


def test_missing_and_placeholder_emails_emit_no_key(base_10: Scenario, standardized: Dbt) -> None:
    """AC3: no `email_exact` row for a record with no usable email (S8.2's two traps)."""
    cfg = config()
    placeholders = set(cfg.standardization.email_placeholders)

    empty: set[str] = set()
    placeholder: set[str] = set()
    for source in SOURCES:
        spec = cfg.sources[source]
        for row in delivered_rows(base_10, source):
            record_key = f"{source}:{row[spec.record_id_column]}"
            delivered = row[spec.columns["email"]].strip()
            if not delivered:
                empty.add(record_key)
            elif delivered in placeholders:
                placeholder.add(record_key)

    # The fixture's own trap index has to agree with what the CSVs deliver, or one of
    # the two is stale and this test would be asserting against the wrong records.
    assert empty == trap_record_keys(base_10, MISSING_EMAILS_TRAP)
    assert placeholder == trap_record_keys(base_10, PLACEHOLDER_EMAIL_TRAP)
    assert len(empty) == 4, sorted(empty)
    assert len(placeholder) == 2, sorted(placeholder)

    with connect() as connection:
        keyed = {
            str(row[0])
            for row in query(
                connection,
                f"SELECT record_key FROM {SCHEMA_QUALIFIER}.{MODEL} WHERE key_type = 'email_exact'",
            )
        }
        values = {
            str(row[0])
            for row in query(
                connection,
                f"SELECT DISTINCT key_value FROM {SCHEMA_QUALIFIER}.{MODEL} "
                f"WHERE key_type = 'email_exact'",
            )
        }

    assert not (keyed & empty), sorted(keyed & empty)
    assert not (keyed & placeholder), sorted(keyed & placeholder)
    # A placeholder address never becomes a key value under ANY record, which is the
    # failure a per-record check alone would miss: `email_norm` nulls it (S4.2), so it
    # cannot reach this relation by any path.
    assert not (values & placeholders), sorted(values & placeholders)
    # And every OTHER record does emit one, so the two assertions above cannot pass by
    # the relation holding no `email_exact` rows at all.
    assert len(keyed) == TOTAL_ROWS - len(empty) - len(placeholder)


def test_drifted_phones_share_one_key(base_10: Scenario, standardized: Dbt) -> None:
    """AC4: three spellings of one number normalize to one key, so the three records block."""
    trap = trap_record_keys(base_10, DRIFTED_PHONES_TRAP)
    assert len(trap) == 3, sorted(trap)

    with connect() as connection:
        phone_keys = {
            str(row[0]): str(row[1])
            for row in query(
                connection,
                f"SELECT record_key, key_value FROM {SCHEMA_QUALIFIER}.{MODEL} "
                f"WHERE key_type = 'phone_exact'",
            )
        }
        blocked_together = {
            str(row[0])
            for row in query(
                connection,
                f"SELECT record_key FROM {SCHEMA_QUALIFIER}.{MODEL} "
                f"WHERE key_type = 'phone_exact' AND key_value = ?",
                DRIFTED_PHONE_E164,
            )
        }

    for_trap = {key: value for key, value in phone_keys.items() if key in trap}
    assert set(for_trap) == trap, sorted(set(for_trap) ^ trap)
    assert set(for_trap.values()) == {DRIFTED_PHONE_E164}, for_trap
    # The other direction, which is the one that means "block together": the key groups
    # those three records and no fourth.
    assert blocked_together == trap, sorted(blocked_together ^ trap)


def test_compiled_sql_is_macro_generated_per_payload_entry(
    standardized: Dbt, tmp_path: Path
) -> None:
    """AC5: one S4.2 branch per payload entry, and a fifth entry needs no model edit."""
    payload = blocking_payload(config())
    branches = compiled_branches()
    assert len(branches) == len(payload), branches

    relation = branch_relation(branches[0])
    assert "int_std_records" in relation, relation
    for entry, branch in zip(payload, branches, strict=True):
        assert branch == expected_branch(entry, relation), branch

    # The model file itself carries no key logic. Asserted against the body with the
    # Jinja header stripped, so a comment that mentions a column is not mistaken for an
    # expression the model would execute.
    body = model_body()
    for entry in payload:
        assert entry["key_type"] not in body, entry["key_type"]
        assert entry["expr"] not in body, entry["expr"]

    # A fifth entry, on a copy of the config. Nothing else changes -- same project, same
    # model file, same macro.
    before = MODEL_SQL.read_text(encoding="utf-8")
    standardized.cfg = config_with_extra_rule(tmp_path / "test-plus-one.yaml")
    extended = blocking_payload(standardized.cfg)
    assert len(extended) == len(payload) + 1

    standardized("run", select=MODEL)

    assert MODEL_SQL.read_text(encoding="utf-8") == before, "the model file was edited"
    recompiled = compiled_branches()
    assert len(recompiled) == len(payload) + 1, recompiled
    for entry, branch in zip(extended, recompiled, strict=True):
        assert branch == expected_branch(entry, relation), branch

    with connect() as connection:
        emitted = key_types(connection)
    assert emitted == set(EXPECTED_KEY_TYPES) | {EXTRA_KEY_TYPE}


def test_delete_insert_replaces_full_key_set_for_touched_record(
    initialised_lake: duckdb.DuckDBPyConnection, standardized: Dbt
) -> None:
    """AC6: a re-run is a no-op, and a changed record loses every prior row it owned (M16)."""
    with connect() as connection:
        before = key_rows(connection)
        count_before = row_count(connection, MODEL)

    standardized.standardize()

    with connect() as connection:
        assert row_count(connection, MODEL) == count_before
        # Not merely the count: the full key set, so a re-run that dropped one key and
        # invented another cannot pass.
        assert key_rows(connection) == before

    with connect() as connection:
        rows = query(
            connection,
            f"SELECT source_record_id, key_value FROM {SCHEMA_QUALIFIER}.{MODEL} "
            f"WHERE source_system = 'crm' AND key_type = 'email_exact' "
            f"ORDER BY source_record_id LIMIT 1",
        )
    assert rows, "base_10 delivers crm records with an email"
    record_id, previous_email = str(rows[0][0]), str(rows[0][1])
    record_key = f"crm:{record_id}"

    append_changed_version(initialised_lake, config(), "crm", record_id)
    standardized.standardize()

    with connect() as connection:
        after = key_rows(connection)
        count_after = row_count(connection, MODEL)

    stale = {row for row in before if row[2] == record_key and row[1] == previous_email}
    assert len(stale) == 1, stale
    (stale_row,) = stale
    fresh = {(stale_row[0], CHANGED_EMAIL, *stale_row[2:])}

    # Exactly one row of the relation moved: the touched record's `email_exact` key. Its
    # prior row is gone -- which is the thing `delete+insert` on the RECORD delivers and
    # a `unique_key` of the logical key would not -- and every other record's rows,
    # including the touched record's other three keys, are untouched.
    assert after == (before - stale) | fresh
    assert count_after == count_before
    assert not any(row[2] == record_key and row[1] == previous_email for row in after)


def test_duplicate_and_unknown_key_type_fail_dbt_tests(
    initialised_lake: duckdb.DuckDBPyConnection, standardized: Dbt
) -> None:
    """AC7: the logical key and the config-derived domain are dbt tests or they are nothing."""
    standardized("test", select=KEYS_SELECTOR)

    # A second row for one `(key_type, key_value, record_key)`. DuckLake enforces no
    # uniqueness, so this lands silently -- which is precisely why S5.0 makes the key a
    # test.
    initialised_lake.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.{MODEL} "
        f"SELECT * FROM {SCHEMA_QUALIFIER}.{MODEL} "
        f"ORDER BY key_type, key_value, record_key LIMIT 1"
    )

    with pytest.raises(StageFailure):
        standardized("test", select=KEYS_SELECTOR)
    assert MODEL in standardized.last_output(), standardized.last_output()

    # Rebuilding restores the invariant, which is what makes the next arm's failure
    # attributable to the unconfigured key type and not to the duplicate still standing.
    # `run` and not `build`, because `build` would run the very tests this test means to
    # interrogate separately.
    standardized("run", select=MODEL)
    standardized("test", select=KEYS_SELECTOR)

    # A row whose `key_type` no `blocking:` entry declares. `* REPLACE` keeps every other
    # value, so the key type is the only thing dbt can be objecting to.
    initialised_lake.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.{MODEL} "
        f"SELECT * REPLACE (? AS key_type) FROM {SCHEMA_QUALIFIER}.{MODEL} "
        f"ORDER BY key_type, key_value, record_key LIMIT 1",
        [UNCONFIGURED_KEY_TYPE],
    )

    with pytest.raises(StageFailure):
        standardized("test", select=MODEL)
    output = standardized.last_output()
    assert DOMAIN_TEST in output, output
