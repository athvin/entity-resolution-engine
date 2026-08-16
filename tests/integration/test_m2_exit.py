"""The M2 exit gate: the milestone's criteria as one collectible module (S12, S8.1, S8.2).

S12 is a gate, and its M2 row is the milestone's only falsifiable exit. This module
adds no coverage of its own: every claim below is already owned by ER-043/044/045/047/
048/049/051 and is *executed* here, in one place, so "M2 is done" is a pytest exit
status rather than a reading of seven ticket files. A failure here is a defect in the
owning ticket and is fixed there — never by weakening an assertion in this file.

**Only relations M2 creates.** S12's gating rule is normative: "a milestone's exit
criteria may name only tests that read relations existing by the end of *that*
milestone". At M2 the lake holds the `ddl.py`-owned fourteen (empty but for
`raw_records` and `ingest_batches`) plus the five dbt-owned relations `staging` and
`intermediate` build. So nothing here reads `match_scores`, `entity_membership`,
`entity_events` or `golden_*`: those are M3's and M4's, and a criterion naming one
would stall an agent on a test it cannot make pass.

**One `M2-EXIT-<k>` tag per criterion.** :data:`M2_EXIT_TAGS` declares the seven, and
:func:`test_exit_tag_set_is_complete` asserts that every declared id is carried by
exactly one test in this module and that every test carries exactly one. A criterion
added without a tag, or a tag without a criterion, fails there — which is what keeps
the tag count an honest statement of how much of the milestone this module runs.

**Where the lake comes from.** From the S8.1 session namespace, through
`initialised_lake`, and never from an attachment of this module's own. The four
criteria that need a corpus each rebuild it: `clean_lake` empties the namespace after
every test by design, so a module-scoped corpus would be a corpus the second test
could not see. dbt is driven through `er.dbt_runner.run_dbt` with the Python
connection detached for the length of every invocation (S4.0b); `er standardize` is
not the entry point because it is still ER-014's `NoOpStage` and would build nothing.

The generator arm runs two real subprocesses under different `PYTHONHASHSEED`s. An
in-process re-call shares the interpreter's hash seed and module state and so proves
almost nothing; S10.1 asks for byte equality across machines, and a `set` or `dict`
iteration order leaking into the corpus is exactly what a second process catches.

Snapshot **counts** are asserted nowhere and may not be: a stage commits a range (S4
preamble), and the ranges this chain commits are not the milestone's claim.
"""

from __future__ import annotations

import inspect
import os
import re
import subprocess
import sys
from collections import Counter
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final

import duckdb
import pytest
from helpers.expected import load_expected
from helpers.scenario import Scenario, load_scenario
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR, DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode, StageFailure
from er.lake.ducklake import attach_statements, connect, detach
from er.lake.hashing import STD_HASH_COLUMNS, table_content_hash
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.api import assert_no_splink_relations_in_lake
from er.matching.model import (
    BLOCKING_DBT_VAR,
    SQL_DIALECT,
    BlockingPayload,
    blocking_rules_from_config,
    build_settings,
)

#: The M2 exit criteria, one entry per criterion. The ids are the module's tags and
#: the text is what each one claims; :func:`test_exit_tag_set_is_complete` is what
#: keeps the two in step.
M2_EXIT_TAGS: Final[Mapping[str, str]] = {
    "M2-EXIT-1": (
        "dbt build --select staging intermediate --target lake is green on base_10 "
        "through run_dbt, and builds exactly the five dbt-owned relations S12 lists "
        "as M2's"
    ),
    "M2-EXIT-2": (
        "int_std_records holds exactly 23 current rows and its per-row std_hash set "
        "equals fixtures/static/base_10/expected/base/std_hashes.csv"
    ),
    "M2-EXIT-3": (
        "int_blocking_keys contains no NULL or empty key_value row, and its key_type "
        "set equals the blocking: key_type set of the loaded config"
    ),
    "M2-EXIT-4": (
        "build_settings(cfg) emits a null level first and an else level last for "
        "every one of the six configured comparisons"
    ),
    "M2-EXIT-5": (
        "zero relations matching __splink__% exist in lake after the M2 chain, and a "
        "relation that does exist makes the guard fail"
    ),
    "M2-EXIT-6": (
        "the generator emits base_10-identical headers and is byte-reproducible "
        "across two processes"
    ),
    "M2-EXIT-7": (
        "the M2-EXIT tag set collected from this module equals the declared seven, "
        "each carried by exactly one test"
    ),
}

#: AC7's literal `{1..7}`. Declared as numbers rather than derived from
#: :data:`M2_EXIT_TAGS`' own keys, so a criterion added to the mapping without a tag
#: on a test — or an eighth criterion nobody meant to add — fails rather than
#: redefining what "complete" means.
DECLARED_TAG_NUMBERS: Final[frozenset[int]] = frozenset(range(1, 8))

#: An `M2-EXIT-<k>` id wherever a test's docstring carries one.
TAG_RE: Final[re.Pattern[str]] = re.compile(r"\bM2-EXIT-\d+\b")

REPO_ROOT: Final[Path] = Path(__file__).resolve().parents[2]

#: The S8.2 fixture this milestone exits on, and its only phase.
SCENARIO_NAME: Final[str] = "base_10"
PHASE: Final[str] = "base"

#: S8.2's ground truth: 23 records over 10 personas, every one of them current.
TOTAL_ROWS: Final[int] = 23

#: The two dbt-owned relations of S4.2, and the selection S12 names.
STD_MODEL: Final[str] = "int_std_records"
BLOCKING_MODEL: Final[str] = "int_blocking_keys"
SELECTOR: Final[str] = "staging intermediate"
TARGET: Final[str] = "lake"

#: S12's "Relations first written" for M2, restricted to the dbt-owned half — the
#: five nodes the selection above must build and no sixth.
EXPECTED_MODEL_NODES: Final[frozenset[str]] = frozenset(
    f"model.er.{name}"
    for name in ("stg_crm", "stg_billing", "stg_webforms", STD_MODEL, BLOCKING_MODEL)
)

#: What dbt reports for a node that did what it was asked: models and seeds succeed,
#: tests pass. Anything else — an error, a failure, a node dbt declined to run — is a
#: red build however the summary line reads.
GREEN_STATUSES: Final[frozenset[str]] = frozenset({"success", "pass"})

#: The `expected/<phase>/<relation>.csv` ER-045 commits for this phase.
STD_HASHES_RELATION: Final[str] = "std_hashes"

#: S6's `comparisons:` block has six entries (S4.3.1), and the bracketing levels are
#: claimed for every one of them.
COMPARISON_COUNT: Final[int] = 6

#: Splink's rendered marker for `ElseLevel()`. It is a literal, not a SQL predicate.
ELSE_CONDITION: Final[str] = "ELSE"

#: The relation AC5's negative arm plants in the lake. It matches `__splink__%`, which
#: is the whole of what the guard looks for, and nothing else in the repo emits it.
PROBE_RELATION: Final[str] = "__splink__probe"

#: The generator's own entry point (S10.1), run as a module from the repo root
#: because `fixtures/` is committed data rather than an installed distribution (S3).
GENERATOR_MODULE: Final[str] = "fixtures.generator.cli"

#: `base_10`'s shape, so the reproducibility arm generates a corpus of the same size
#: as the fixture whose headers it is compared against. Small enough that two
#: subprocess emissions stay inside the integration budget.
GENERATOR_PERSONAS: Final[int] = 10
GENERATOR_RECORDS: Final[int] = TOTAL_ROWS

#: Every file one emission writes at the root of `--out` (S10.1).
CORPUS_FILES: Final[tuple[str, ...]] = ("crm.csv", "billing.csv", "webforms.csv", "truth.csv")

#: Two hash seeds, deliberately different. With the default randomisation two runs
#: could agree by luck; disagreement is the expected outcome for any generator that
#: leaks an iteration order into the corpus.
HASH_SEEDS: Final[tuple[str, str]] = ("0", "12345")


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


def scalar(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: Any) -> Any:
    rows = query(connection, sql, *parameters)
    assert rows, f"{sql!r} returned no row"
    return rows[0][0]


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


def computed_hashes(connection: duckdb.DuckDBPyConnection) -> dict[tuple[str, str], str]:
    """`(source_system, source_record_id) -> std_hash` over the current corpus.

    The projection is `STD_HASH_COLUMNS` in its literal T-STD-1 order plus the two key
    columns, and the digest is `table_content_hash` (ER-044) — the same function the
    determinism arm uses, so the two cannot disagree about what a `std_hash` is.
    """
    projection = ("source_system", "source_record_id", *STD_HASH_COLUMNS)
    rows = query(connection, f"SELECT {', '.join(projection)} FROM {SCHEMA_QUALIFIER}.{STD_MODEL}")

    computed: dict[tuple[str, str], str] = {}
    for row in rows:
        values: Mapping[str, Any] = dict(zip(projection, row, strict=True))
        key = (str(values["source_system"]), str(values["source_record_id"]))
        assert key not in computed, f"{key}: two current rows for one record (S4.2)"
        computed[key] = table_content_hash(values)
    return computed


def emit_corpus_subprocess(out_dir: Path, *, seed: int, hash_seed: str) -> None:
    """One corpus, emitted by a fresh interpreter under an explicit `PYTHONHASHSEED`."""
    completed = subprocess.run(
        [
            sys.executable,
            "-m",
            GENERATOR_MODULE,
            "--personas",
            str(GENERATOR_PERSONAS),
            "--records",
            str(GENERATOR_RECORDS),
            "--seed",
            str(seed),
            "--out",
            str(out_dir),
            "--config",
            os.environ["ER_CONFIG"],
        ],
        cwd=REPO_ROOT,
        env=dict(os.environ, PYTHONHASHSEED=hash_seed),
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


def header_of(path: Path) -> str:
    """The first line of a CSV, as committed and as emitted."""
    return path.read_text(encoding="utf-8").splitlines()[0]


def tags_by_test() -> dict[str, list[str]]:
    """Every `M2-EXIT-<k>` id each test function in this module carries, by name."""
    found: dict[str, list[str]] = {}
    for name, function in sorted(globals().items()):
        if not name.startswith("test_") or not inspect.isfunction(function):
            continue
        found[name] = TAG_RE.findall(inspect.getdoc(function) or "")
    return found


@dataclass(frozen=True)
class Dbt:
    """dbt as `er standardize` will invoke it: real `--vars`, no connection spanning it.

    The `blocking:` payload rides in `extra` under `er.matching.model.BLOCKING_DBT_VAR`,
    which is what makes `int_blocking_keys` buildable at all: the model is macro
    generated from that var, and the var comes from S4.2's one generator rather than
    from a second rendering of the config.
    """

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
            target=TARGET,
            close_conn=lambda: detach(self.connection),
            reopen_conn=self._reattach,
            project_dir=DBT_PROJECT_DIR,
            profiles_dir=DBT_PROFILES_DIR,
            artifacts_dir=self.artifacts,
        )

    def _reattach(self) -> None:
        for statement in attach_statements():
            self.connection.execute(statement)


@dataclass(frozen=True)
class Chain:
    """One executed M2 chain: the harness that ran it and what the build reported."""

    dbt: Dbt
    build: DbtResult


@pytest.fixture
def base_10() -> Scenario:
    """The S8.2 fixture, opened through ER-028's loader."""
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (PHASE,)
    return scenario


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    """`dbt deps`, unless the packages are already vendored.

    The image ships no `dbt/dbt_packages` (`.dockerignore`) and both intermediate
    models carry `dbt_utils` tests. A plain subprocess: `deps` touches no warehouse,
    so it is the one invocation that is not a stage.
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
def m2_chain(
    base_10: Scenario,
    dbt_packages: None,
    initialised_lake: duckdb.DuckDBPyConnection,
    tmp_path: Path,
) -> Chain:
    """`base_10` ingested and standardized on this test's fresh namespace.

    The seed runs first and is part of neither selector: `name_variants` reaches the
    `nickname_variants` seed through `ref()` (S4.2), and a `--select staging
    intermediate` build does not carry a seed into its selection.
    """
    delivery = deliver(base_10, tmp_path / "drop")
    for source in base_10.inputs_for(PHASE):
        result = run_er("ingest", "--source", source, "--path", str(delivery))
        assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    dbt = Dbt(connection=initialised_lake, artifacts=tmp_path / "artifacts", cfg=config())
    dbt("seed")
    return Chain(dbt=dbt, build=dbt("build", select=SELECTOR))


def test_dbt_build_staging_and_intermediate_green(m2_chain: Chain) -> None:
    """M2-EXIT-1 / AC1: the S12 selection builds green on `base_10` through `run_dbt`,
    and materializes exactly the five dbt-owned relations M2 first writes."""
    assert m2_chain.build.exit_code == 0

    failed = sorted(
        (run.unique_id, run.status)
        for run in m2_chain.build.models
        if run.status not in GREEN_STATUSES
    )
    assert not failed, f"nodes that did not succeed: {failed}"

    # Set equality, not containment: a sixth dbt-owned relation under either selector
    # would be a relation S12 does not list among M2's, and an absent one would make
    # every row assertion below vacuous for want of a table.
    built = {run.unique_id for run in m2_chain.build.models if run.unique_id.startswith("model.")}
    assert built == set(EXPECTED_MODEL_NODES), sorted(built ^ set(EXPECTED_MODEL_NODES))

    # Read back on a connection of its own: S12 is explicit that a command's own
    # output does not satisfy a criterion, so what is asserted is what reached the lake.
    with connect() as connection:
        for relation in (STD_MODEL, BLOCKING_MODEL):
            assert (
                int(scalar(connection, f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{relation}")) > 0
            ), f"{relation} built green and empty"


def test_int_std_records_matches_expected_std_hashes(base_10: Scenario, m2_chain: Chain) -> None:
    """M2-EXIT-2 / AC2: 23 current rows, and the digest set ER-045 committed."""
    with connect() as connection:
        computed = computed_hashes(connection)
        rows = int(scalar(connection, f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{STD_MODEL}"))

    committed_path = base_10.expected_path(PHASE, STD_HASHES_RELATION)
    assert committed_path is not None, (
        f"expected/{PHASE}/{STD_HASHES_RELATION}.csv is not committed; M2 has no digest "
        "set to exit on"
    )
    committed = {
        (str(row["source_system"]), str(row["source_record_id"])): str(row["std_hash"])
        for row in load_expected(committed_path)
    }

    assert rows == TOTAL_ROWS, f"{rows} rows in {STD_MODEL}, expected {TOTAL_ROWS}"
    assert len(computed) == TOTAL_ROWS
    assert len(committed) == TOTAL_ROWS

    # Set equality in both directions, with the symmetric difference printed: what a
    # fixture author has to act on is WHICH triples moved, and a bare equality over 23
    # SHA-256s prints two walls of hex.
    computed_set = {(*key, digest) for key, digest in computed.items()}
    committed_set = {(*key, digest) for key, digest in committed.items()}
    only_computed = sorted(computed_set - committed_set)
    only_committed = sorted(committed_set - computed_set)
    assert not (only_computed or only_committed), (
        f"std_hash sets differ.\n"
        f"  computed but not committed ({len(only_computed)}): {only_computed}\n"
        f"  committed but not computed ({len(only_committed)}): {only_committed}\n"
        f"This is a defect in the ticket that owns int_std_records or the expectation, "
        f"not something the exit gate resolves."
    )


def test_blocking_keys_have_no_null_or_empty_values(m2_chain: Chain) -> None:
    """M2-EXIT-3 / AC3: S4.2's NULL/empty policy on every row, and a key_type set the
    loaded config decides completely."""
    configured = {entry["key_type"] for entry in blocking_payload(config())}
    assert configured, "the config declares no blocking rule; the comparison below is vacuous"

    with connect() as connection:
        offending = int(
            scalar(
                connection,
                f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{BLOCKING_MODEL} "
                f"WHERE key_value IS NULL OR key_value = ''",
            )
        )
        by_type = {
            str(key): int(count)
            for key, count in query(
                connection,
                f"SELECT key_type, count(*) FROM {SCHEMA_QUALIFIER}.{BLOCKING_MODEL} "
                f"GROUP BY key_type",
            )
        }

    assert offending == 0
    # Derived from the config rather than transcribed, per AC3 — and non-vacuous in
    # both directions: every configured rule emitted rows, and no row carries a key
    # type no rule declares.
    assert set(by_type) == configured, sorted(set(by_type) ^ configured)
    assert all(count > 0 for count in by_type.values()), by_type


def test_settings_builder_brackets_every_comparison() -> None:
    """M2-EXIT-4 / AC4: `NullLevel` first and `ElseLevel()` last on all six comparisons.

    A pure claim about the config, so it opens no connection: S4.3.1 makes both levels
    unconditional, and without the else level Splink renders a `CASE … END` with no
    `ELSE` — gamma NULL and a NULL match weight, silently, for the pairs that score
    lowest.
    """
    cfg = config()
    settings = build_settings(cfg).create_settings_dict(SQL_DIALECT)
    comparisons = settings["comparisons"]

    assert len(cfg.comparisons) == COMPARISON_COUNT, sorted(cfg.comparisons)
    assert len(comparisons) == COMPARISON_COUNT

    for comparison in comparisons:
        column = str(comparison["output_column_name"])
        levels = comparison["comparison_levels"]
        conditions = [str(level["sql_condition"]) for level in levels]

        assert levels[0]["is_null_level"] is True, f"{column}: first level is not the null level"
        # On its OWN column: a null test on some other column is still a null level,
        # and would make the whole comparison unreachable for the wrong pairs.
        assert conditions[0] == f'"{column}_l" IS NULL OR "{column}_r" IS NULL'
        assert conditions[-1] == ELSE_CONDITION, f"{column}: no else level"
        assert ELSE_CONDITION not in conditions[:-1], f"{column}: an else level before the last"


def test_no_splink_relations_reach_the_lake(
    initialised_lake: duckdb.DuckDBPyConnection, m2_chain: Chain
) -> None:
    """M2-EXIT-5 / AC5: the guard passes after the full M2 chain, and a planted
    `__splink__` relation makes it fail.

    The negative arm is the reason to believe the positive one. Nothing in M2 runs
    Splink at all — the settings builder is pure and the scorer is M3's — so a guard
    that could only ever pass would read exactly like a green criterion here.
    """
    assert_no_splink_relations_in_lake(initialised_lake)

    initialised_lake.execute(f"CREATE TABLE {SCHEMA_QUALIFIER}.{PROBE_RELATION} (probe VARCHAR)")
    try:
        with pytest.raises(StageFailure) as refusal:
            assert_no_splink_relations_in_lake(initialised_lake)
        assert PROBE_RELATION in str(refusal.value)
    finally:
        initialised_lake.execute(f"DROP TABLE {SCHEMA_QUALIFIER}.{PROBE_RELATION}")

    # And the lake is clean again, so the arm above left nothing for the next test to
    # inherit and the criterion is asserted on the state the chain actually produced.
    assert_no_splink_relations_in_lake(initialised_lake)


def test_generator_is_reproducible_and_header_identical(base_10: Scenario, tmp_path: Path) -> None:
    """M2-EXIT-6 / AC6: two processes, one seed, identical bytes — and the headers are
    `base_10`'s."""
    seed = config().generator.seed
    emissions = [tmp_path / "corpus-a", tmp_path / "corpus-b"]
    for destination, hash_seed in zip(emissions, HASH_SEEDS, strict=True):
        emit_corpus_subprocess(destination, seed=seed, hash_seed=hash_seed)

    first, second = emissions
    for name in CORPUS_FILES:
        assert (first / name).read_bytes() == (second / name).read_bytes(), (
            f"{name} differs between two processes at seed {seed}"
        )

    committed = base_10.inputs_for(PHASE)
    emitted_sources = {name.removesuffix(".csv") for name in CORPUS_FILES} - {"truth"}
    assert set(committed) == emitted_sources, sorted(set(committed) ^ emitted_sources)
    for source, path in committed.items():
        emitted_header = header_of(first / f"{source}.csv")
        assert emitted_header == header_of(path), (
            f"the generated {source}.csv header is not the committed base_10 header; "
            f"S6 owns the mapping, so the config and the fixture move together (M14)"
        )
        assert emitted_header, f"{source}.csv was emitted without a header row"


def test_exit_tag_set_is_complete() -> None:
    """M2-EXIT-7 / AC7: the declared seven, each enforced by exactly one test.

    Ids are read off the test functions rather than out of the module's source text,
    because :data:`M2_EXIT_TAGS`' own keys are ids too: a textual scan would count
    every criterion once for its declaration and call an untagged criterion tagged.
    """
    assert {int(tag.rsplit("-", 1)[1]) for tag in M2_EXIT_TAGS} == set(DECLARED_TAG_NUMBERS), (
        f"the declared criteria are {sorted(M2_EXIT_TAGS)}, which is not the literal "
        f"M2-EXIT set {sorted(DECLARED_TAG_NUMBERS)}"
    )

    by_test = tags_by_test()
    untagged = sorted(name for name, tags in by_test.items() if len(tags) != 1)
    assert not untagged, (
        f"{untagged} carry other than exactly one M2-EXIT id; one criterion per test is "
        "what makes the tag count a true statement of what this module runs"
    )

    # Missing, duplicated and unknown ids are all one comparison: a criterion enforced
    # twice is as wrong as a criterion enforced nowhere.
    tagged = Counter(tag for tags in by_test.values() for tag in tags)
    assert tagged == Counter(M2_EXIT_TAGS.keys()), (
        f"tags {sorted(tagged.items())} do not match the declared criteria {sorted(M2_EXIT_TAGS)}"
    )
