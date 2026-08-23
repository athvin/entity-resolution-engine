"""`base_10`'s designed traps, verified against the committed model (S8.2, S8.3).

The S8.2 trap table describes properties of the fixture **under the committed model**.
They cannot be asserted where the fixture is authored — "robert/bob merges" is a claim
about a Splink score, not about a CSV — so until now they were relied upon by six
downstream tests (T-MATCH-1a/1b, T-REVIEW-1, T-GOLD-1 and the two incremental arms) and
checked by none. A fixture edit that dissolved a trap would surface as a quality metric
drifting somewhere else entirely, which is design gap M21.

This module makes them a post-match gate: `base_10` is ingested, standardized and scored
at `fixtures/static/model_test_v1.json`, and each row of the S8.2 table is then asserted
by name under a `TRAP-<name>` id. Two things beyond the table are asserted here as well,
because S8.2 marks both **normative authoring constraints** rather than observations:

* the gray-band pair MUST be cross-persona — a same-persona one leaves that persona split
  across two entities, so T-MATCH-1b's `entity count == 10` fails with precision still
  `1.0` and nothing pointing at the cause;
* a tolerated missed edge MUST lie inside a persona of three or more records and stay
  transitively recoverable — inside a 2-record persona the missed edge is the persona's
  only edge, and the same silent failure shape returns.

The survivorship tie and the recency precondition are asserted here too, over
`int_std_records` and the config chains. T-GOLD-1 will depend on both being true, and
`golden_records` does not exist until M4, so this is the earliest point at which either
can be checked at all (the ticket's DoD forbids reading `golden_records`,
`golden_lineage` or `entity_membership` from here).

**Ground truth is read, never the pipeline's answer.** Persona membership comes from
`truth.csv` through :mod:`helpers.traps`, never from `entity_membership`: a trap asserted
against the pipeline's own clustering would be asserting the pipeline against itself.

**A failing trap is a blocker against ER-041, not a patch here.** `fixtures/static/base_10/`,
`src/er/matching/` and `configs/test.yaml` are all `protected_paths` of this ticket. If a
trap fails, the fixture or the model is wrong; making the assertion agree with it would
delete the only thing standing between a corpus regression and six green tests.

The `Dbt` harness and the delivery helper are duplicated from
`tests/integration/test_full_match.py` rather than imported, for the reason that module
states: a test module importing another test module makes a node id's dependencies
invisible to whoever reads it.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.model import fixture_settings, load_fixture_model
from helpers.pairs import Pair, pair_key_types
from helpers.scenario import Scenario, load_scenario
from helpers.traps import (
    load_trap_index,
    persona_members,
    persona_of_record,
    true_pairs_from_truth,
)
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import TERMINAL_SURVIVORSHIP_RULE, Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.entities.ids import canonicalize_pair
from er.errors import ExitCode
from er.lake.ducklake import attach_statements, detach
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.full import MATCH_SCORES_RELATION, score_full
from er.matching.model import BLOCKING_DBT_VAR, BlockingPayload, blocking_rules_from_config
from er.matching.thresholds import in_gray_band, is_auto_merge
from er.obs.counters import DECLARED_COUNTERS, StageCounters
from er.obs.runctx import StageRun

MODULE_PATH: Final = Path(__file__).resolve()

#: The S8.2 fixture this suite scores, and its only phase.
SCENARIO_NAME: Final = "base_10"
PHASE: Final = "base"

#: The two dbt selectors `er standardize` runs (S4.2).
STAGING_SELECTOR: Final = "staging"
INTERMEDIATE_SELECTOR: Final = "intermediate"

#: The scenario-root ground truth this module reads (S8.2.1).
TRUTH_FILE: Final = "truth.csv"
TRAPS_FILE: Final = "traps.csv"

MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.{MATCH_SCORES_RELATION}"
STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"
BLOCKING_KEYS: Final = f"{SCHEMA_QUALIFIER}.int_blocking_keys"

#: The stage name S5 records match work under.
MATCH_STAGE: Final = "match"

#: The eight rows of the S8.2 designed-traps table, spelled as the ids this module
#: asserts under. `test_trap_ids_cover_the_s8_2_table` compares this set against the
#: committed `traps.csv` index — the machine-readable form of that table — so a trap
#: added to the spec and the fixture fails the suite until an assertion exists.
NICKNAME_PAIR: Final = "nickname_pair"
TYPO_SURNAME: Final = "typo_surname"
SHARED_HOUSEHOLD: Final = "shared_household"
MISSING_EMAILS: Final = "missing_emails"
DRIFTED_PHONES: Final = "drifted_phones"
PLACEHOLDER_EMAIL: Final = "placeholder_email"
SURVIVORSHIP_TIE: Final = "survivorship_tie"
GRAY_BAND_PAIR: Final = "gray_band_pair"

TRAP_IDS: Final[frozenset[str]] = frozenset(
    f"TRAP-{name}"
    for name in (
        NICKNAME_PAIR,
        TYPO_SURNAME,
        SHARED_HOUSEHOLD,
        MISSING_EMAILS,
        DRIFTED_PHONES,
        PLACEHOLDER_EMAIL,
        SURVIVORSHIP_TIE,
        GRAY_BAND_PAIR,
    )
)

#: The single normalized form S8.2 requires the three drifted spellings to collapse to.
DRIFTED_PHONE_E164: Final = "+14155550132"

#: The `key_type`s of S6's `blocking:` block this module names directly.
EMAIL_EXACT: Final = "email_exact"
PHONE_EXACT: Final = "phone_exact"

#: The survivorship rule whose `ORDER BY` fragment is
#: `COALESCE(updated_at_source, ingested_at) DESC` (S4.6). `ingested_at` is a
#: `VOLATILE_COLUMNS` member (S5.0), so a contest the COALESCE decides is
#: non-deterministic across runs — which is what AC7 forbids on `base_10`.
RECENCY: Final = "recency"
VALIDATED: Final = "validated"
SOURCE_PRIORITY: Final = "source_priority"

#: Which `int_std_records` column the `validated` rule reads, per attribute (S4.6:
#: `<attr>_valid DESC NULLS LAST`). `address` has no validity column, which is why
#: `validated` never precedes `recency` in its chain.
VALIDITY_COLUMN: Final[Mapping[str, str]] = {"email": "email_valid", "phone_e164": "phone_valid"}


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


def rows_as_dicts(
    connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any
) -> list[dict[str, Any]]:
    """Every row of ``statement`` keyed by its result column names."""
    cursor = connection.execute(statement, list(parameters))
    names = [str(description[0]) for description in cursor.description or ()]
    return [dict(zip(names, row, strict=True)) for row in cursor.fetchall()]


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


@pytest.fixture
def truth(base_10: Scenario) -> Path:
    """The committed `truth.csv`, the only membership these assertions read."""
    return base_10.truth[TRUTH_FILE]


@pytest.fixture
def traps(base_10: Scenario) -> Mapping[str, tuple[str, ...]]:
    """The committed trap index: trap name -> the record keys constructing it."""
    return load_trap_index(base_10.truth[TRAPS_FILE])


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

    The teardown restores the connection's default schema, for the reason
    `test_full_match.py` states: `splink_api` issues ``SET schema 'splink_scratch'`` on
    whatever connection it is handed, and this one is the session handle every other
    suite shares.
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
def scored(standardized: duckdb.DuckDBPyConnection, cfg: Config) -> duckdb.DuckDBPyConnection:
    """`base_10` scored once at the committed model — the state every score trap reads.

    Only the three tests that assert scores request this. The blocking, tie and recency
    traps are properties of `int_std_records` and `int_blocking_keys`, and making them
    pay for a Splink pass they do not read would add minutes to the PR path for nothing.
    """
    model_version, tf_snapshot_id, _ = load_fixture_model(standardized)
    stage_run = StageRun(
        run_id=str(ULID()),
        stage=MATCH_STAGE,
        seq=1,
        started_at=datetime.now(UTC),
        counters=StageCounters(DECLARED_COUNTERS[MATCH_STAGE]),
    )
    result = score_full(
        standardized,
        cfg,
        stage_run,
        model_version=model_version,
        tf_snapshot_id=tf_snapshot_id,
        settings=fixture_settings(),
    )
    assert result.pairs_scored > 0, "the stage scored nothing; every trap below is vacuous"
    return standardized


def scored_pairs(connection: duckdb.DuckDBPyConnection) -> dict[Pair, float]:
    """Every `match_scores` row as canonical pair -> `match_probability`."""
    rows = connection.execute(
        f"SELECT rec_a_key, rec_b_key, match_probability FROM {MATCH_SCORES}"
    ).fetchall()
    return {
        canonicalize_pair(str(rec_a), str(rec_b)): float(probability)
        for rec_a, rec_b, probability in rows
    }


def describe(pair: Pair, scores: Mapping[Pair, float], personas: Mapping[str, str]) -> str:
    """One pair, with its probability and both persona labels — the failure line.

    The DoD requires a trap failure to print the offending `record_key`s and
    probabilities rather than a boolean, because the first question anyone asks of a
    broken trap is "by how much, and between which records".
    """
    probability = scores.get(pair)
    reading = "absent from match_scores" if probability is None else f"p={probability:.6f}"
    return (
        f"{pair[0]} ({personas.get(pair[0], '?')}) | "
        f"{pair[1]} ({personas.get(pair[1], '?')}): {reading}"
    )


def connected(members: Sequence[str], edges: set[Pair]) -> bool:
    """Whether ``edges`` connects every one of ``members`` into a single component."""
    if len(members) <= 1:
        return True
    adjacency: dict[str, set[str]] = {member: set() for member in members}
    for left, right in edges:
        if left in adjacency and right in adjacency:
            adjacency[left].add(right)
            adjacency[right].add(left)
    seen = {members[0]}
    frontier = [members[0]]
    while frontier:
        for neighbour in adjacency[frontier.pop()]:
            if neighbour not in seen:
                seen.add(neighbour)
                frontier.append(neighbour)
    return seen == set(members)


def test_exactly_one_gray_band_pair_and_it_is_cross_persona(
    scored: duckdb.DuckDBPyConnection,
    cfg: Config,
    truth: Path,
    traps: Mapping[str, tuple[str, ...]],
) -> None:
    """TRAP-gray_band_pair, AC1: one banded pair, and it spans two personas.

    The cross-persona half is S8.2's normative authoring constraint, not an observation.
    A same-persona banded pair would split that persona into two entities — T-MATCH-1b's
    `entity count == 10` fails while precision stays `1.0`, and nothing says why.
    """
    scores = scored_pairs(scored)
    personas = persona_of_record(truth)

    banded = sorted(
        pair for pair, probability in scores.items() if in_gray_band(probability, cfg.thresholds)
    )
    # The whole scored distribution, not just the band: when the band is EMPTY the
    # banded list prints nothing, and "0 != 1" alone cannot tell a reader whether the
    # designed pair scored above `auto_merge`, below `review_low`, or was never
    # blocked at all. Those are three different defects with three different fixes.
    designed = canonicalize_pair(*traps[GRAY_BAND_PAIR])
    distribution = "\n".join(
        f"  {describe(pair, scores, personas)}"
        for pair in sorted(scores, key=lambda key: (-scores[key], key))
    )
    assert len(banded) == 1, (
        f"{len(banded)} pairs fall in the half-open gray band "
        f"[{cfg.thresholds.review_low}, {cfg.thresholds.auto_merge}); S8.2 designs exactly one.\n"
        f"banded: {banded}\n"
        f"the pair traps.csv designs for the band: {describe(designed, scores, personas)}\n"
        f"every scored pair, descending:\n{distribution}"
    )

    pair = banded[0]
    assert personas[pair[0]] != personas[pair[1]], (
        "the gray-band pair is same-persona, which S8.2 forbids as an authoring "
        "constraint: it would leave that persona split across two entities and fail "
        f"T-MATCH-1b's entity count with precision still 1.0. {describe(pair, scores, personas)}"
    )
    assert set(pair) == set(traps[GRAY_BAND_PAIR]), (
        f"the banded pair is {pair}, but traps.csv indexes gray_band_pair to "
        f"{traps[GRAY_BAND_PAIR]}; the fixture and the model disagree about which pair "
        "the band holds"
    )


def test_at_least_one_true_pair_is_single_rule_covered(
    standardized: duckdb.DuckDBPyConnection, truth: Path
) -> None:
    """AC2: some true pair is carried by exactly one `key_type`.

    A corpus in which every true pair is reachable by two or more rules cannot show a
    blocking regression: drop a rule and the pair survives on the other one, so
    `int_blocking_keys` shrinks and every quality metric stays put. At least one
    single-rule-covered true pair is what makes T-BLK-1 load-bearing.
    """
    key_types = pair_key_types(standardized)
    true_pairs = true_pairs_from_truth(truth)

    covered = {pair: key_types[pair] for pair in sorted(true_pairs) if pair in key_types}
    assert covered, "no true pair is blocked at all; the blocking rules cover none of the truth"

    single = {pair: types[0] for pair, types in covered.items() if len(types) == 1}
    assert single, (
        "every blocked true pair is carried by two or more key_types, so no blocking "
        "regression is visible in this fixture. Coverage:\n"
        + "\n".join(f"  {a} | {b}: {','.join(types)}" for (a, b), types in covered.items())
    )
    # The DoD asks for the pair and the rule by name, not a count.
    print(
        "single-rule-covered true pairs:\n"
        + "\n".join(f"  {a} | {b}  via {rule}" for (a, b), rule in sorted(single.items()))
    )


def test_missed_true_pairs_satisfy_the_authoring_constraint(
    scored: duckdb.DuckDBPyConnection, cfg: Config, truth: Path
) -> None:
    """AC3: any tolerated missed edge is inside a persona of >=3 and stays recoverable.

    S8.2 makes this normative because T-MATCH-1a tolerates one missed pair while
    T-MATCH-1b asserts `entity count == 10` and precision `1.0` on the same corpus.
    Those are jointly satisfiable only when the missed edge is transitively recoverable.

    Zero missed pairs satisfies the assertion, and is S8.2's preferred authoring. The
    loop is written so that a later fixture edit introducing one is checked by an
    assertion that already exists rather than by one nobody remembered to add.
    """
    scores = scored_pairs(scored)
    personas = persona_of_record(truth)
    members_by_persona = persona_members(truth)
    true_pairs = true_pairs_from_truth(truth)

    missed = sorted(
        pair for pair in true_pairs if not is_auto_merge(scores.get(pair, 0.0), cfg.thresholds)
    )
    # The DoD requires the offending keys and probabilities, not a count: which pairs
    # were missed and by how much is the whole content of a fixture-authoring failure.
    print(
        f"true pairs: {len(true_pairs)}; missed at auto_merge={cfg.thresholds.auto_merge}: "
        f"{len(missed)}\n" + "\n".join(f"  {describe(pair, scores, personas)}" for pair in missed)
    )

    for pair in missed:
        persona = personas[pair[0]]
        assert personas[pair[1]] == persona, f"true pair spans personas: {pair}"
        members = members_by_persona[persona]
        assert len(members) >= 3, (
            f"missed true pair lies in {persona}, which has {len(members)} record(s); "
            "S8.2 requires every below-auto_merge true pair to lie inside a persona of "
            f"three or more, or the persona splits and T-MATCH-1b fails. "
            f"{describe(pair, scores, personas)}"
        )
        surviving = {
            other
            for other in true_pairs
            if other != pair
            and personas[other[0]] == persona
            and is_auto_merge(scores.get(other, 0.0), cfg.thresholds)
        }
        assert connected(members, surviving), (
            f"removing {pair} disconnects {persona}: its remaining >= auto_merge edges "
            f"are {sorted(surviving)}, which do not span {list(members)}. The pair is "
            "not transitively recoverable, so the persona becomes two entities."
        )


def test_named_score_traps_hold_under_committed_model(
    scored: duckdb.DuckDBPyConnection,
    cfg: Config,
    truth: Path,
    traps: Mapping[str, tuple[str, ...]],
) -> None:
    """TRAP-nickname_pair, TRAP-typo_surname, TRAP-shared_household, TRAP-placeholder_email.

    AC4, the four traps whose required outcome is a score: two that MUST merge, one that
    MUST NOT, and the placeholder address that must be nulled before it can pair anything.
    """
    scores = scored_pairs(scored)
    personas = persona_of_record(truth)

    for trap in (NICKNAME_PAIR, TYPO_SURNAME):
        left, right = traps[trap]
        pair = canonicalize_pair(left, right)
        assert is_auto_merge(scores.get(pair, 0.0), cfg.thresholds), (
            f"TRAP-{trap} must merge at auto_merge={cfg.thresholds.auto_merge} "
            f"(S8.2): {describe(pair, scores, personas)}"
        )

    household = traps[SHARED_HOUSEHOLD]
    crossing = sorted(
        pair
        for pair in scores
        if set(pair) <= set(household) and is_auto_merge(scores[pair], cfg.thresholds)
    )
    assert not crossing, (
        "TRAP-shared_household must NOT merge: two personas at one address, with no "
        "shared email, phone or birth_date, may have zero edges at or above "
        "auto_merge. Found:\n"
        + "\n".join(f"  {describe(pair, scores, personas)}" for pair in crossing)
    )

    placeholder = traps[PLACEHOLDER_EMAIL]
    emails = rows_as_dicts(
        scored,
        f"SELECT record_key, email FROM {STD_RECORDS} WHERE record_key IN "
        f"({', '.join('?' for _ in placeholder)})",
        *placeholder,
    )
    assert len(emails) == len(placeholder), (
        f"expected {len(placeholder)} placeholder-email rows in int_std_records, "
        f"found {len(emails)}"
    )
    for row in emails:
        assert row["email"] is None, (
            f"TRAP-placeholder_email: {row['record_key']} kept email {row['email']!r}; "
            "`email_norm` nulls every value in standardization.email_placeholders "
            f"({cfg.standardization.email_placeholders}, S4.2)"
        )
    placeholder_pair = canonicalize_pair(*placeholder)
    assert placeholder_pair not in scores, (
        "TRAP-placeholder_email: the two test@test.com records were scored against each "
        f"other, so the nulled address still formed a component. "
        f"{describe(placeholder_pair, scores, personas)}"
    )


def test_phone_drift_and_missing_email_blocking_traps(
    standardized: duckdb.DuckDBPyConnection, traps: Mapping[str, tuple[str, ...]]
) -> None:
    """TRAP-drifted_phones and TRAP-missing_emails, AC5: blocking-layer traps.

    Both are properties of standardization and `int_blocking_keys`, so neither needs a
    score. `NullLevel` firing is asserted as the absence of a key rather than as a
    comparison level: a null that still emitted an `email_exact` row would block every
    record sharing the empty string, which is the failure the trap exists to catch.
    """
    drifted = traps[DRIFTED_PHONES]
    rows = rows_as_dicts(
        standardized,
        f"SELECT record_key, phone_e164 FROM {STD_RECORDS} WHERE record_key IN "
        f"({', '.join('?' for _ in drifted)}) ORDER BY record_key",
        *drifted,
    )
    assert len(rows) == len(drifted), f"expected {len(drifted)} drifted-phone rows, got {len(rows)}"
    normalised = {row["record_key"]: row["phone_e164"] for row in rows}
    assert set(normalised.values()) == {DRIFTED_PHONE_E164}, (
        f"TRAP-drifted_phones: the three spellings normalised to {normalised}, not all "
        f"to {DRIFTED_PHONE_E164} (S8.2)"
    )

    key_values = rows_as_dicts(
        standardized,
        f"SELECT record_key, key_value FROM {BLOCKING_KEYS} WHERE key_type = ? AND "
        f"record_key IN ({', '.join('?' for _ in drifted)})",
        PHONE_EXACT,
        *drifted,
    )
    blocked = {row["record_key"]: row["key_value"] for row in key_values}
    assert set(blocked) == set(drifted), (
        f"TRAP-drifted_phones: only {sorted(blocked)} emitted a {PHONE_EXACT} key; all "
        f"of {list(drifted)} must block together"
    )
    assert len(set(blocked.values())) == 1, (
        f"TRAP-drifted_phones: the three records carry different {PHONE_EXACT} key "
        f"values {blocked}, so they do not block together"
    )

    empty_email = traps[MISSING_EMAILS]
    stored = rows_as_dicts(
        standardized,
        f"SELECT record_key, email FROM {STD_RECORDS} WHERE record_key IN "
        f"({', '.join('?' for _ in empty_email)})",
        *empty_email,
    )
    assert len(stored) == len(empty_email)
    non_null = [row for row in stored if row["email"] is not None]
    assert not non_null, f"TRAP-missing_emails: {non_null} carry an email; all four are empty"

    leaked = rows_as_dicts(
        standardized,
        f"SELECT record_key, key_value FROM {BLOCKING_KEYS} WHERE key_type = ? AND "
        f"record_key IN ({', '.join('?' for _ in empty_email)})",
        EMAIL_EXACT,
        *empty_email,
    )
    assert not leaked, (
        f"TRAP-missing_emails: {len(leaked)} {EMAIL_EXACT} rows were emitted for records "
        f"with no email — a null key must never block (S4.2): {leaked}"
    )


def test_survivorship_tie_row_exists(
    standardized: duckdb.DuckDBPyConnection,
    cfg: Config,
    truth: Path,
    traps: Mapping[str, tuple[str, ...]],
) -> None:
    """TRAP-survivorship_tie, AC6: exactly one persona holds the designed tie.

    The tie is what forces S4.6's mandatory terminal `record_key ASC` to decide, which
    is the only reason T-GOLD-1's winner is deterministic. Asserted structurally over
    `int_std_records`: `golden_records` does not exist until M4.
    """
    rows = rows_as_dicts(
        standardized,
        f"SELECT record_key, source_system, given_name, "
        f"COALESCE(updated_at_source, ingested_at) AS recency_key FROM {STD_RECORDS}",
    )
    by_key = {row["record_key"]: row for row in rows}
    tying: dict[str, list[Pair]] = {}
    for persona, members in persona_members(truth).items():
        for index, left in enumerate(members):
            for right in members[index + 1 :]:
                a, b = by_key[left], by_key[right]
                if a["source_system"] != b["source_system"]:
                    continue
                if a["recency_key"] != b["recency_key"]:
                    continue
                if a["given_name"] is None or b["given_name"] is None:
                    continue
                if len(str(a["given_name"])) != len(str(b["given_name"])):
                    continue
                if a["given_name"] == b["given_name"]:
                    continue
                tying.setdefault(persona, []).append(canonicalize_pair(left, right))

    assert len(tying) == 1, (
        f"{len(tying)} personas hold a survivorship tie; S8.2 designs exactly one. Found: {tying}"
    )
    (persona, pairs) = next(iter(tying.items()))
    assert len(pairs) == 1, f"{persona} holds {len(pairs)} tying pairs, expected 1: {pairs}"
    assert set(pairs[0]) == set(traps[SURVIVORSHIP_TIE]), (
        f"the tie is {pairs[0]}, but traps.csv indexes survivorship_tie to "
        f"{traps[SURVIVORSHIP_TIE]}"
    )

    # Same source, so `source_priority` cannot separate them either — stated rather
    # than inferred, because it is half of why the terminal rule has to decide.
    left, right = pairs[0]
    ranks = {
        by_key[key]["source_system"]: cfg.sources[str(by_key[key]["source_system"])].priority_rank
        for key in (left, right)
    }
    assert len(set(ranks.values())) == 1, f"the tying rows have different priority_rank: {ranks}"
    assert TERMINAL_SURVIVORSHIP_RULE in cfg.survivorship["given_name"], (
        "the terminal `record_key ASC` element is missing from the given_name chain, so "
        "nothing decides this tie deterministically (S4.6)"
    )


def _prefix_key(rule: str, attribute: str, row: Mapping[str, Any], cfg: Config) -> Any:
    """The comparable value ``rule`` contributes for ``row``, for chain elements before recency.

    Only the rules that can actually precede `recency` in an S6 chain are implemented.
    Anything else raises rather than returning a constant: a rule silently treated as
    "does not separate" would make AC7 assert less than it claims, which is precisely
    the failure mode a trap test exists to prevent.
    """
    if rule == VALIDATED:
        column = VALIDITY_COLUMN.get(attribute)
        assert column is not None, (
            f"`validated` precedes `recency` in the {attribute} chain, but S4.6 gives "
            f"{attribute} no <attr>_valid column to read"
        )
        # `DESC NULLS LAST`: True sorts before False sorts before NULL.
        value = row[column]
        return (value is None, not bool(value))
    if rule == SOURCE_PRIORITY:
        return cfg.sources[str(row["source_system"])].priority_rank
    raise AssertionError(
        f"survivorship rule {rule!r} precedes `recency` in the {attribute} chain and "
        "this test cannot evaluate it; AC7 would under-assert. Implement it here."
    )


def test_recency_never_decides_on_ingested_at(
    standardized: duckdb.DuckDBPyConnection, cfg: Config, truth: Path
) -> None:
    """AC7: no survivorship contest is decided by the `ingested_at` fallback.

    `recency`'s fragment is `COALESCE(updated_at_source, ingested_at) DESC` (S4.6) and
    `ingested_at` is a `VOLATILE_COLUMNS` member (S5.0), so a contest the COALESCE
    resolves is decided by *when the fixture happened to be loaded*. T-GOLD-1 would then
    be non-deterministic across runs for a reason invisible in its own assertion.

    The chains are read from the config rather than re-listed, so adding `recency` to an
    attribute widens this assertion with no edit here.
    """
    rows = rows_as_dicts(
        standardized,
        f"SELECT record_key, source_system, updated_at_source, email_valid, phone_valid "
        f"FROM {STD_RECORDS}",
    )
    by_key = {row["record_key"]: row for row in rows}
    recency_attributes = sorted(
        attribute for attribute, chain in cfg.survivorship.items() if RECENCY in chain
    )
    assert recency_attributes, "no survivorship chain contains `recency`; AC7 is vacuous"

    for attribute in recency_attributes:
        chain = cfg.survivorship[attribute]
        prefix = chain[: chain.index(RECENCY)]
        for persona, members in persona_members(truth).items():
            groups: dict[tuple[Any, ...], list[str]] = {}
            for key in members:
                bucket = tuple(_prefix_key(rule, attribute, by_key[key], cfg) for rule in prefix)
                groups.setdefault(bucket, []).append(key)
            for bucket, contenders in groups.items():
                if len(contenders) < 2:
                    continue
                fallback = sorted(
                    key for key in contenders if by_key[key]["updated_at_source"] is None
                )
                assert not fallback, (
                    f"{attribute}: in {persona}, {fallback} reach the `recency` element "
                    f"with a NULL updated_at_source, so COALESCE falls back to the "
                    f"volatile `ingested_at` to decide against {sorted(contenders)}. "
                    f"The chain prefix {list(prefix)} did not separate them "
                    f"(bucket {bucket})."
                )


def test_trap_ids_cover_the_s8_2_table(traps: Mapping[str, tuple[str, ...]]) -> None:
    """AC8: the ids this module asserts are exactly the S8.2 designed-traps table.

    `traps.csv` is that table in machine-readable form (S8.2.1), so the comparison is
    against the fixture rather than against a list re-typed from the spec. Adding a trap
    to S8.2 and to the fixture therefore fails here until an assertion exists.

    The source-text check is the other half: :data:`TRAP_IDS` is a literal, and a
    declared id that no test mentions would satisfy the set comparison while asserting
    nothing.
    """
    indexed = {f"TRAP-{name}" for name in traps}
    assert TRAP_IDS == indexed, (
        "the asserted trap ids and the committed trap index disagree.\n"
        f"  declared but not in {TRAPS_FILE}: {sorted(TRAP_IDS - indexed)}\n"
        f"  in {TRAPS_FILE} but not asserted: {sorted(indexed - TRAP_IDS)}"
    )
    assert len(TRAP_IDS) == 8, f"S8.2 designs eight traps; this module declares {len(TRAP_IDS)}"

    source = MODULE_PATH.read_text(encoding="utf-8")
    unmentioned = sorted(trap_id for trap_id in TRAP_IDS if trap_id not in source)
    assert not unmentioned, (
        f"{unmentioned} are declared in TRAP_IDS but appear in no assertion in this "
        "module; a declared id that nothing asserts is a trap nobody is checking"
    )

    for name, members in traps.items():
        assert members, f"traps.csv indexes {name} to no records"
