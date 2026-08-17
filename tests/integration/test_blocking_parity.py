"""T-BLK-1: the dbt key table and Splink block the same pairs on `base_10` (S4.2, S8.3).

The blocking logic has three descriptions — the `blocking:` block of S6, the macro-
generated `int_blocking_keys` model, and the `block_on` rules Splink's inference passes
receive — and exactly one generation direction, `blocking_rules_from_config(cfg)`. This
file is what makes the mirror checkable: on a real `base_10` corpus, the DISTINCT
canonicalised pair set derived from `int_blocking_keys` equals Splink's blocked pair set
exactly, in both directions, with the symmetric difference printed when it does not.
Without it, drift between the two consumers is silent and surfaces later as a T-INC-1
failure attributed to clustering.

Both derivations live in `tests/helpers/pairs.py` and neither is repeated here: two
copies of "the pair set" drift the same way the blocking logic does, and the parity
check would then be comparing two implementations of one mistake.

**What the negative arm does, and why it does it that way.** AC1 requires that an
injected divergence fails *and* reports usefully, so the test deletes one `key_type`'s
rows from `int_blocking_keys` and recomputes the dbt side. That is the state a lost
`UNION ALL` branch leaves — the payload no longer carries the entry, so the relation no
longer carries its rows — reached without a second dbt invocation whose `delete+insert`
would have to be `--full-refresh`ed to drop the branch's existing rows anyway. The
Splink side is deliberately *not* recomputed: what T-BLK-1 catches is one side moving.

**Path note (S8.3).** S8.3 lists T-BLK-1's file as `tests/integration/test_blocking.py`;
the board pins `tests/integration/test_blocking_parity.py`, which is where it ships. The
node name `test_dbt_and_splink_pair_sets_match` is S8.3's own and is unchanged, so
ER-103's node-id lint has one path deviation to record and no name to reconcile.

The `Dbt` harness and the delivery helper are duplicated from
`tests/integration/test_full_match.py` rather than imported, for the reason that module
and `tests/integration/scenarios/test_blocking_keys.py` both state: a test module
importing another test module makes a node id's dependencies invisible to whoever reads
it.
"""

from __future__ import annotations

import csv
import os
import subprocess
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import load_fixture_model
from helpers.pairs import (
    BLOCKING_KEYS_RELATION,
    Pair,
    blocking_key_pair_rows,
    canonical_pairs_from_blocking_keys,
    pair_key_types,
    splink_blocked_pairs,
    symmetric_difference_report,
)
from helpers.scenario import Scenario, load_scenario
from splink import block_on
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.api import assert_no_splink_relations_in_lake, leaked_splink_relations
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config

#: The S8.2 fixture this suite blocks, and its only phase. `base_10` and not a generated
#: corpus: S8.2 assigns blocking parity to it, and its designed traps are what make the
#: NULL-key arm below non-vacuous.
SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

#: The two dbt selectors `er standardize` runs (S4.2).
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

BLOCKING_KEYS: Final = f"{SCHEMA_QUALIFIER}.{BLOCKING_KEYS_RELATION}"

#: The `key_type` carrying the two records whose email `email_norm` nulls (S8.2's
#: `placeholder_email` trap). AC4 asserts they reach no pair of it on either side.
EMAIL_KEY_TYPE: Final = "email_exact"

#: The S8.2 trap indexing those two records, and the file it is indexed in (S8.2.1).
PLACEHOLDER_EMAIL_TRAP: Final = "placeholder_email"
TRAPS_FILE: Final = "traps.csv"


def config() -> Config:
    """The validated S6 document this session runs against (S7.1)."""
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


def deliver(scenario: Scenario, root: Path) -> Path:
    """Materialise the scenario's phase as the drop-folder root `er ingest --path` reads."""
    for source, path in scenario.inputs_for(PHASE).items():
        directory = root / source
        directory.mkdir(parents=True, exist_ok=True)
        (directory / path.name).write_bytes(path.read_bytes())
    return root


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


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


def pairs_only_from(attribution: dict[Pair, tuple[str, ...]], key_type: str) -> frozenset[Pair]:
    """The pairs no `key_type` but ``key_type`` carries — the ones dropping it would lose."""
    return frozenset(pair for pair, types in attribution.items() if types == (key_type,))


def rule_exprs(cfg: Config) -> list[str]:
    """The `expr` string every `BlockingRuleCreator` in the generator's list was built from.

    Read off `block_on`'s own `ColumnExpression` rather than re-rendered: S4.2 puts the
    byte-identity at the `block_on` *input*, and the rendered rule is deliberately not
    the input — `block_on` re-renders through sqlglot and qualifies each column with
    `l.` / `r.` inside the expression. The raw expression is the only place the string
    survives, which is what makes AC2 a claim about one generator call rather than about
    two renderings agreeing.
    """
    _, rules = blocking_rules_from_config(cfg)
    return [str(rule.col_expression.raw_sql_expression) for rule in rules]


@dataclass
class Dbt:
    """dbt as a stage invokes it: real `--vars`, no connection spanning it (S4.0b)."""

    connection: duckdb.DuckDBPyConnection
    artifacts: Path
    cfg: Config

    def __call__(self, command: str, select: str | None = None) -> DbtResult:
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
            artifacts_dir=self.artifacts,
        )

    def standardize(self) -> None:
        """The selection `er standardize` runs: staging, then intermediate (S4.2)."""
        self("build", select=STAGING_SELECTOR)
        self("build", select=INTERMEDIATE_SELECTOR)

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


@pytest.fixture(scope="session")
def cfg() -> Config:
    """The S6 document Compose supplies as `ER_CONFIG` (S7.1)."""
    return config()


@pytest.fixture
def base_10() -> Scenario:
    """The S8.2 fixture, opened through ER-028's loader."""
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (PHASE,)
    return scenario


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored.

    The image ships no `dbt/dbt_packages` (`.dockerignore`) and the intermediate build
    runs `dbt_utils` tests. A plain subprocess: `deps` touches no warehouse.
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
def standardized(
    base_10: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    cfg: Config,
    tmp_path: Path,
) -> Iterator[duckdb.DuckDBPyConnection]:
    """`base_10` ingested and standardized on this test's fresh namespace.

    The teardown restores the connection's default schema. `splink_api` issues
    ``SET schema 'splink_scratch'`` on whatever connection it is handed, and this one is
    the session handle every other suite shares — leaving it repointed would make a
    later test's unqualified statement land somewhere it did not choose.
    """
    delivery = deliver(base_10, tmp_path / "drop")
    for source in base_10.inputs_for(PHASE):
        result = run_er("ingest", "--source", source, "--path", str(delivery))
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=cfg)
    dbt("seed")
    dbt.standardize()

    database = str(scalar(initialised_lake, "SELECT current_database()"))
    schema = str(scalar(initialised_lake, "SELECT current_schema()"))
    try:
        yield initialised_lake
    finally:
        initialised_lake.execute(f'USE "{database}".{schema}')


@pytest.fixture
def settings(standardized: duckdb.DuckDBPyConnection) -> dict[str, Any]:
    """The committed model document, installed into this lake (S4.3.2 item 6).

    A scenario test does not fit a model (S4.3.2 item 6), and this one has no reason to
    want one: T-BLK-1 asserts nothing about a probability, and the only thing the
    document contributes is a comparison model for the linker to exist with. Loaded
    rather than hand-built because the committed artifact is what every other `base_10`
    suite scores under — so the corpus Splink blocks here is the corpus it blocks when
    it scores.
    """
    _, _, document = load_fixture_model(standardized)
    return document


def splink_pairs(
    connection: duckdb.DuckDBPyConnection, cfg: Config, settings: dict[str, Any]
) -> set[Pair]:
    """Splink's blocked pair set, asserted non-empty before any comparison uses it.

    An empty set on either side makes every set assertion in this file pass or fail for
    the wrong reason, so the emptiness check is here rather than repeated per test.
    """
    pairs = splink_blocked_pairs(connection, cfg, settings)
    assert pairs, "Splink blocked nothing on base_10; every parity assertion would be vacuous"
    return pairs


def test_dbt_and_splink_pair_sets_match(
    standardized: duckdb.DuckDBPyConnection, cfg: Config, settings: dict[str, Any]
) -> None:
    """AC1: set equality in both directions, and a real divergence is reported per pair."""
    dbt_pairs = canonical_pairs_from_blocking_keys(standardized)
    splink = splink_pairs(standardized, cfg, settings)
    attribution = pair_key_types(standardized)

    assert dbt_pairs, "int_blocking_keys implied no pairs; the comparison would be vacuous"
    report = symmetric_difference_report(dbt_pairs, splink, key_types=attribution)
    # Both directions, named separately, because the two failures mean different things:
    # a pair dbt has and Splink does not is a rule the macro over-generates for, and the
    # reverse is a `key_type` the model never emitted (S4.2).
    assert not (dbt_pairs - splink), report
    assert not (splink - dbt_pairs), report
    assert dbt_pairs == splink, report

    # The injected divergence. One `key_type`'s rows are removed from the relation — the
    # state a payload that lost its entry produces — and the comparison must now fail and
    # say which pairs went and which rule carried them.
    carried = {key_type for types in attribution.values() for key_type in types}
    droppable = sorted(key_type for key_type in carried if pairs_only_from(attribution, key_type))
    assert droppable, "no key_type carries a pair of its own; the divergence would be invisible"
    dropped = droppable[0]
    lost = pairs_only_from(attribution, dropped)

    standardized.execute(f"DELETE FROM {BLOCKING_KEYS} WHERE key_type = ?", [dropped])
    diverged = canonical_pairs_from_blocking_keys(standardized)

    assert diverged != splink, f"dropping {dropped!r} changed no pair; the arm proves nothing"
    assert splink - diverged == lost
    divergence = symmetric_difference_report(diverged, splink, key_types=attribution)
    for rec_a_key, rec_b_key in sorted(lost):
        located = [
            line
            for line in divergence.splitlines()
            if rec_a_key in line and rec_b_key in line and f"key_type={dropped}" in line
        ]
        assert located, f"{rec_a_key} | {rec_b_key} is unattributed in:\n{divergence}"


def test_both_sides_come_from_one_generator_call(cfg: Config) -> None:
    """AC2: the dbt payload and the Splink rules were built from the same `expr` strings."""
    payload, rules = blocking_rules_from_config(cfg)

    assert payload and len(payload) == len(rules)
    payload_exprs = [entry["expr"] for entry in payload]
    assert set(payload_exprs) == set(rule_exprs(cfg))
    # Order too, and not merely as a set: Splink attributes a pair to the first rule that
    # produced it, so a reordering changes which rule the exclusion chain credits even
    # though the pair set is unchanged (S4.2).
    assert payload_exprs == rule_exprs(cfg)

    # And the strings reached `block_on` unmodified: a rule rebuilt from the payload's
    # own `expr` is the same rule. Compared through the rendered SQL because that is what
    # `block_on` produces and what Splink consumes.
    dialect = "duckdb"
    for entry, rule in zip(payload, rules, strict=True):
        rebuilt = block_on(entry["expr"])
        assert (
            rule.get_blocking_rule(dialect).blocking_rule_sql
            == rebuilt.get_blocking_rule(dialect).blocking_rule_sql
        ), entry


def test_pairs_are_canonical_and_self_free(
    standardized: duckdb.DuckDBPyConnection, cfg: Config, settings: dict[str, Any]
) -> None:
    """AC3: `rec_a_key < rec_b_key` on both sides, and neither holds a self-pair (S5.0)."""
    dbt_pairs = canonical_pairs_from_blocking_keys(standardized)
    splink = splink_pairs(standardized, cfg, settings)

    assert dbt_pairs
    for label, pairs in ((BLOCKING_KEYS_RELATION, dbt_pairs), ("splink", splink)):
        for rec_a_key, rec_b_key in pairs:
            assert rec_a_key != rec_b_key, f"{label}: self-pair on {rec_a_key}"
            assert rec_a_key < rec_b_key, f"{label}: {rec_a_key} !< {rec_b_key}"


def test_null_and_empty_keys_never_block(
    base_10: Scenario,
    standardized: duckdb.DuckDBPyConnection,
    cfg: Config,
    settings: dict[str, Any],
) -> None:
    """AC4: no NULL or empty key exists, and the nulled emails block on neither side."""
    offending = scalar(
        standardized,
        f"SELECT count(*) FROM {BLOCKING_KEYS} WHERE key_value is null OR key_value = ''",
    )
    assert offending == 0
    # Non-vacuous: two of the configured expressions concatenate columns that ARE missing
    # on some `base_10` records, which is what the `<> ''` half of the predicate is for.
    assert scalar(standardized, f"SELECT count(*) FROM {BLOCKING_KEYS}") > 0

    placeholder = trap_record_keys(base_10, PLACEHOLDER_EMAIL_TRAP)
    assert len(placeholder) == 2, sorted(placeholder)

    # The dbt side: those two records carry no `email_exact` row at all, so no
    # `email_exact` pair can name them.
    keyed = {
        str(record_key)
        for (record_key,) in standardized.execute(
            f"SELECT record_key FROM {BLOCKING_KEYS} WHERE key_type = ?", [EMAIL_KEY_TYPE]
        ).fetchall()
    }
    assert not (keyed & placeholder), sorted(keyed & placeholder)

    email_pairs = {
        pair for pair, types in pair_key_types(standardized).items() if EMAIL_KEY_TYPE in types
    }
    assert email_pairs, "no email_exact pair exists; the exclusion below would be vacuous"
    assert not [pair for pair in email_pairs if set(pair) & placeholder]

    # The Splink side. Splink's blocked set carries no `key_type`, so "no `email_exact`
    # pair" is asked of a config holding that rule ALONE: every pair the run returns is
    # then an email pair by construction, which a filter over the four-rule set could not
    # claim. It also re-runs the mirror at one rule — the same parity, localised.
    email_only = cfg.model_copy(
        update={"blocking": [rule for rule in cfg.blocking if rule.key_type == EMAIL_KEY_TYPE]}
    )
    assert len(email_only.blocking) == 1, email_only.blocking
    splink_email = splink_blocked_pairs(standardized, email_only, settings)

    assert not [pair for pair in splink_email if set(pair) & placeholder]
    assert splink_email == email_pairs, symmetric_difference_report(
        email_pairs, splink_email, key_types=pair_key_types(standardized)
    )


def test_multiplicity_collapses_to_one_pair(standardized: duckdb.DuckDBPyConnection) -> None:
    """AC5: a pair two `key_type`s carry is returned once, and the raw join returns it more."""
    rows = blocking_key_pair_rows(standardized)
    pairs = canonical_pairs_from_blocking_keys(standardized)
    attribution = pair_key_types(standardized)

    multiply_carried = sorted(pair for pair, types in attribution.items() if len(types) > 1)
    assert multiply_carried, (
        "no base_10 pair is carried by two key_types; the DISTINCT requirement of S4.2 "
        "would be untested on this fixture"
    )

    pair = multiply_carried[0]
    carried = [row for row in rows if (row[0], row[1]) == pair]
    assert len(carried) > 1, carried
    assert len({row[2] for row in carried}) == len(attribution[pair]) > 1
    assert sum(1 for candidate in pairs if candidate == pair) == 1
    # And in aggregate: the un-DISTINCTed join is strictly larger than the derived set,
    # which is the difference S4.2 says the two candidate sets would otherwise show.
    assert len(rows) > len(pairs)


def test_no_splink_relations_in_lake(
    standardized: duckdb.DuckDBPyConnection, cfg: Config, settings: dict[str, Any]
) -> None:
    """AC6: blocking a real corpus through `splink_api` leaves nothing of Splink's in the lake."""
    assert splink_pairs(standardized, cfg, settings)

    materialized = scalar(
        standardized,
        "SELECT count(*) FROM duckdb_tables() WHERE database_name = current_database() "
        "AND table_name LIKE '__splink__%'",
    )
    assert materialized > 0, "Splink materialized nothing; the leak check would be vacuous"

    assert leaked_splink_relations(standardized) == ()
    assert_no_splink_relations_in_lake(standardized)
