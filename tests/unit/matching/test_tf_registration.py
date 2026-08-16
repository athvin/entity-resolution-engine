"""D4's registration contract and the two confinements that make it a policy (S4.3.3).

Frozen TF is only as good as the calls that are *not* made. Registering the frozen
rows is worth nothing if some other path computes term frequency from the corpus at
hand, and a `tf_snapshot_id` minted outside the two paths D4 sanctions moves pairs
across `auto_merge` with no row recording that a boundary was crossed. Neither of
those is visible in the output of a passing pipeline, so both are asserted here as
source scans over `src/er/`, `fixtures/` and `tests/` — the guard AC7 asks for, as a
test rather than as a comment.

The registration test uses a spy linker rather than a real `Linker` for the same
reason: AC4's claim is partly negative ("and never invokes Splink's own TF
computation"), and a negative about which calls were made can only be asserted against
something that records every call it receives. The spy's `__getattr__` is what makes
it record calls nobody wrote — including the one D4 forbids.

Nothing here needs Docker. `lake.main.tf_lookup` is an in-memory database ATTACHed
under the lake's alias and created from `REGISTRY`'s own `TableSpec`, so the table
these tests read is the table S5 declares rather than a hand-typed copy of it.
"""

from __future__ import annotations

import re
from collections.abc import Iterator, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import duckdb
import pytest
import yaml
from ulid import ULID

from er.config.loader import load_config
from er.config.schema import Config
from er.lake.model import REGISTRY, create_table_sql
from er.matching.tf import (
    TF_COLUMN_PREFIX,
    TF_LOOKUP_RELATION,
    MissingTfLookupError,
    materialize_tf_lookup,
    parse_tf_tables_path,
    register_tf,
    tf_columns,
    tf_tables_path,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

#: The three `comparisons` keys S6 marks `tf: true`, in document order. Spelled out
#: rather than read from the config: a test deriving its expectation from the file
#: under test asserts nothing about that file.
EXPECTED_TF_COLUMNS = ("given_name", "family_name", "email")

#: The three that are configured and are NOT TF columns. Their presence is what makes
#: "reads only `tf: true`" a claim rather than a restatement of `comparisons.keys()`.
EXPECTED_NON_TF_COLUMNS = ("phone_e164", "birth_date", "addr_postal")

#: The Splink entry point D4 forbids every scoring path from calling, and the mint D4
#: confines to two callers. Held as strings and turned into call patterns below, so
#: this module never contains a call site of either and needs no exemption from its
#: own scan.
COMPUTE_TF = "compute_tf_table"
MINT = "new_tf_snapshot_id"

#: The roots AC7 names. `.py` only: both tokens are Python API names, and a match in a
#: CSV fixture or a `.sql` model would be a coincidence rather than a call.
SCAN_ROOTS = ("src/er", "fixtures", "tests")

#: Where a call to Splink's TF computation is permitted, and why. Not "wherever it is
#: convenient": each entry is a place D4's confinement demonstrably does not reach.
#: `src/er/` is absent on purpose — no production path may hold one at all, which is
#: what :func:`test_compute_tf_table_has_one_call_site` asserts separately.
COMPUTE_TF_CALL_SITES: Mapping[str, str] = {
    "tests/integration/test_splink_isolation.py": (
        "ER-049's negative arm deliberately misconstructs a DuckDBAPI and computes one "
        "TF table, to prove `assert_no_splink_relations_in_lake` fires at all. It scores "
        "nothing and writes no match_scores row, so INV-SCORE is not in its scope."
    ),
}

#: Where a `tf_snapshot_id` may be minted, and why. The two production entries are the
#: two D4 names; neither module exists yet (ER-054, ER-055), and the scan tolerates
#: that rather than pinning the guard to the order the board happens to run in.
MINT_CALL_SITES: Mapping[str, str] = {
    "src/er/matching/tf.py": "the definition, and the single mint site D4 requires",
    "src/er/matching/train.py": "`er train` mints when it materializes tf_lookup (D4)",
    "src/er/matching/full.py": "`er match --mode full --new-tf-snapshot`: the `er correct` path",
    "tests/integration/test_tf_schema.py": (
        "the materialization suite, which must have a snapshot id to materialize under. "
        "D4 confines the mint in the pipeline; a test that never scores cannot cross a "
        "TF boundary a match_scores row would have to record."
    ),
}


def call_sites(token: str) -> dict[str, tuple[int, ...]]:
    """Every `<token>(` in :data:`SCAN_ROOTS`, as repo-relative path -> line numbers.

    A call pattern rather than a bare occurrence: the tokens name functions, and the
    thing D4 confines is invoking them. Matching the bare name would also flag the
    prose in this file and in the module under test, and a guard that has to exempt
    every mention of its own subject stops being readable long before it stops being
    correct.
    """
    pattern = re.compile(rf"\b{re.escape(token)}\s*\(")
    found: dict[str, tuple[int, ...]] = {}
    for root in SCAN_ROOTS:
        for source in sorted((REPO_ROOT / root).rglob("*.py")):
            if "__pycache__" in source.parts:
                continue
            lines = source.read_text(encoding="utf-8").splitlines()
            hits = tuple(n for n, line in enumerate(lines, 1) if pattern.search(line))
            if hits:
                found[str(source.relative_to(REPO_ROOT))] = hits
    return found


def assert_allowlist_is_not_stale(token: str, allowed: Mapping[str, str]) -> None:
    """Every allowlisted path that exists must actually hold a call site.

    Without this the allowlist only ever grows: an exemption whose call site was
    deleted keeps standing, and the next module to land at that path inherits a
    permission nobody granted it. Paths that do not exist yet are skipped — two of the
    mint's are modules later tickets create.
    """
    sites = call_sites(token)
    for path in allowed:
        if not (REPO_ROOT / path).exists():
            continue
        assert path in sites, (
            f"{path} is allowlisted for {token} but holds no call site; "
            f"remove the entry rather than leaving a standing exemption"
        )


def document() -> dict[str, Any]:
    """A fresh, mutable copy of the shipped test document."""
    loaded = yaml.safe_load(TEST_CONFIG.read_text(encoding="utf-8"))
    assert isinstance(loaded, dict), "configs/test.yaml is not a mapping"
    return loaded


@dataclass
class SpyCall:
    """One call the spy received: what was invoked, and with what."""

    name: str
    kwargs: dict[str, Any]


@dataclass
class SpyTableManagement:
    """A `linker.table_management` that records every call, named or not.

    The `__getattr__` fallback is the point: a method this class does not declare is
    still recorded and still returns, so the assertion "Splink's TF computation was
    never invoked" is made against evidence rather than against the absence of a
    `NameError`.
    """

    calls: list[SpyCall] = field(default_factory=list)

    def register_term_frequency_lookup(
        self, input_data: Any, col_name: str, overwrite: bool = False
    ) -> None:
        self.calls.append(
            SpyCall(
                "register_term_frequency_lookup",
                {"input_data": input_data, "col_name": col_name, "overwrite": overwrite},
            )
        )

    def __getattr__(self, name: str) -> Any:
        def recorder(*args: Any, **kwargs: Any) -> None:
            self.calls.append(SpyCall(name, kwargs))

        return recorder


@dataclass
class SpyLinker:
    """A `Linker`, narrowed to the surface :func:`register_tf` touches."""

    table_management: SpyTableManagement = field(default_factory=SpyTableManagement)


@pytest.fixture
def cfg() -> Config:
    """The validated S6 document `configs/test.yaml` holds."""
    return load_config(TEST_CONFIG)


@pytest.fixture
def lake() -> Iterator[duckdb.DuckDBPyConnection]:
    """An in-memory stand-in for the lake, holding S5's own `tf_lookup`.

    ATTACHed under the alias rather than created as `main.tf_lookup`: every statement
    in the module under test is written `lake.main.…` because S4.0b forbids DuckLake
    being the default catalog, and a fixture that made the table reachable unqualified
    would let a missing qualifier pass here and fail against a real lake.
    """
    connection = duckdb.connect()
    connection.execute("ATTACH ':memory:' AS lake")
    connection.execute(create_table_sql(REGISTRY[TF_LOOKUP_RELATION]))
    try:
        yield connection
    finally:
        connection.close()


def freeze(
    connection: duckdb.DuckDBPyConnection,
    model_version: str,
    tf_snapshot_id: str,
    rows: list[tuple[str, str, float]],
) -> None:
    """Insert `(column_name, value, tf_value)` rows under one key, directly.

    The unit layer has no `int_std_records` to compute from, and the registration
    contract is about what is read back, not about how it got there — the computation
    is `tests/integration/test_tf_schema.py`'s subject.
    """
    connection.executemany(
        "INSERT INTO lake.main.tf_lookup VALUES (?, ?, ?, ?, ?)",
        [(model_version, tf_snapshot_id, column, value, tf) for column, value, tf in rows],
    )


def test_tf_columns_reads_only_tf_true(cfg: Config) -> None:
    """AC1: `tf_columns` is the `tf: true` keys in config order, and nothing else."""
    assert tf_columns(cfg) == EXPECTED_TF_COLUMNS

    # The other three are configured comparisons: `tf_columns` returning them would be
    # invisible until a scoring run registered TF for a column S4.3.1 forbids it on.
    for column in EXPECTED_NON_TF_COLUMNS:
        assert column in cfg.comparisons, f"{column} is not in the document under test"
        assert column not in tf_columns(cfg)

    # A level list cannot turn TF on: S4.3.1 puts the flag on the attribute, and the
    # builder reaches only the exact-match level with it.
    doc = document()
    doc["comparisons"]["phone_e164"]["levels"] = ["exact", "null"]
    assert "phone_e164" not in tf_columns(Config.model_validate(doc))


def test_register_tf_calls_register_term_frequency_lookup_per_column(
    cfg: Config, lake: duckdb.DuckDBPyConnection
) -> None:
    """AC4: one registration per TF column, in order, and no other call at all."""
    model_version = "v0001"
    tf_snapshot_id = str(ULID())
    freeze(
        lake,
        model_version,
        tf_snapshot_id,
        [(column, f"{column}-value", 0.25) for column in EXPECTED_TF_COLUMNS],
    )
    linker = SpyLinker()

    registered = register_tf(linker, lake, cfg, model_version, tf_snapshot_id)

    assert registered == EXPECTED_TF_COLUMNS
    calls = linker.table_management.calls
    assert [call.name for call in calls] == ["register_term_frequency_lookup"] * len(
        EXPECTED_TF_COLUMNS
    ), f"the spy received {[call.name for call in calls]}; D4 sanctions exactly one method"
    assert [call.kwargs["col_name"] for call in calls] == list(EXPECTED_TF_COLUMNS)

    # The frame Splink joins on is read by column name, so the shape is the interface.
    for call, column in zip(calls, EXPECTED_TF_COLUMNS, strict=True):
        frame = call.kwargs["input_data"]
        assert frame == [{column: f"{column}-value", f"{TF_COLUMN_PREFIX}{column}": 0.25}]


def test_register_tf_refuses_a_key_missing_a_column(
    cfg: Config, lake: duckdb.DuckDBPyConnection
) -> None:
    """AC5's unit arm: a partially frozen key is a precondition failure, not a subset.

    Registering two of three columns is worse than registering none: Splink would
    compute the third from whatever corpus is at hand, so the resulting probability is
    neither the frozen value nor a reproducible one.
    """
    model_version = "v0001"
    tf_snapshot_id = str(ULID())
    freeze(lake, model_version, tf_snapshot_id, [("given_name", "ada", 0.5)])
    linker = SpyLinker()

    with pytest.raises(MissingTfLookupError) as refusal:
        register_tf(linker, lake, cfg, model_version, tf_snapshot_id)

    assert "family_name" in str(refusal.value), "the refusal must name the missing column"
    assert refusal.value.code == 3
    # Whatever it registered before it refused is irrelevant; what matters is that it
    # did not fall through to Splink computing the rest.
    assert all(
        call.name == "register_term_frequency_lookup" for call in linker.table_management.calls
    )


def test_tf_tables_path_round_trips() -> None:
    """AC6: the producer and the parser are inverses, and a malformed string raises."""
    model_version = "v0007"
    tf_snapshot_id = str(ULID())

    path = tf_tables_path(model_version, tf_snapshot_id)

    assert path == (
        f"lake.main.{TF_LOOKUP_RELATION}"
        f"?model_version={model_version}&tf_snapshot_id={tf_snapshot_id}"
    )
    assert parse_tf_tables_path(path) == (model_version, tf_snapshot_id)

    malformed = [
        "",
        "lake.main.tf_lookup",
        f"lake.main.tf_lookup?model_version={model_version}",
        f"lake.main.tf_lookup?tf_snapshot_id={tf_snapshot_id}&model_version={model_version}",
        f"lake.main.match_scores?model_version={model_version}&tf_snapshot_id={tf_snapshot_id}",
        f"lake.main.tf_lookup?model_version=&tf_snapshot_id={tf_snapshot_id}",
    ]
    for candidate in malformed:
        with pytest.raises(ValueError):
            parse_tf_tables_path(candidate)

    # The producer refuses what it could not parse back, so the inverse is total over
    # everything it can return rather than over everything it is handed.
    for bad in ("", "v0001&x", "v0001?x", "v0001=x"):
        with pytest.raises(ValueError):
            tf_tables_path(bad, tf_snapshot_id)


def test_compute_tf_table_has_one_call_site() -> None:
    """AC7: no production path computes term frequency, and the exception is named.

    D4 permits the call only from `materialize_tf_lookup`; this implementation does not
    need it there either, computing the frozen rows in SQL from `int_std_records`, so
    the production count is zero. The single allowlisted site is a test that proves a
    *different* guard fires and scores nothing.
    """
    sites = call_sites(COMPUTE_TF)

    unexpected = {path: lines for path, lines in sites.items() if path not in COMPUTE_TF_CALL_SITES}
    assert not unexpected, (
        f"{COMPUTE_TF} is called at {unexpected}. S4.3.3 forbids every incremental and full "
        f"run from computing term frequency: the same pair would then score differently in a "
        f"base run, an incremental run and a full re-resolution, and INV-SCORE would be false. "
        f"Register the frozen rows with er.matching.tf.register_tf instead."
    )
    assert not [path for path in sites if path.startswith("src/er/")], (
        "a module under src/er/ computes term frequency; D4 confines that to "
        "materialize_tf_lookup, which does not need it"
    )
    assert_allowlist_is_not_stale(COMPUTE_TF, COMPUTE_TF_CALL_SITES)


def test_tf_snapshot_id_mint_is_confined() -> None:
    """AC7: the mint is called only where D4 says a new TF boundary may be created."""
    sites = call_sites(MINT)

    unexpected = {path: lines for path, lines in sites.items() if path not in MINT_CALL_SITES}
    assert not unexpected, (
        f"{MINT} is called at {unexpected}. D4 mints a tf_snapshot_id in `er train` and, "
        f"outside it, only in `er correct` via `er match --mode full --new-tf-snapshot`. Any "
        f"other mint moves pairs across auto_merge with no match_scores row recording that a "
        f"TF boundary was crossed."
    )
    assert "src/er/matching/tf.py" in sites, "the mint must live in the module that owns it"
    assert_allowlist_is_not_stale(MINT, MINT_CALL_SITES)


def test_materialize_refuses_a_column_int_std_records_does_not_have(
    lake: duckdb.DuckDBPyConnection,
) -> None:
    """A `tf: true` key that is not an S5 column is a config rejection, not a SQL error.

    The column name reaches an identifier position in the aggregate, so it is the one
    config-authored string this module interpolates. S6.1 V6 rejects it at load time;
    this is the same rejection for a `Config` built without the loader.
    """
    doc = document()
    doc["comparisons"]["not_a_column"] = {"levels": ["exact"], "tf": True}

    with pytest.raises(Exception) as refusal:
        materialize_tf_lookup(lake, Config.model_validate(doc), "v0001", str(ULID()))

    assert "not_a_column" in str(refusal.value)
    assert getattr(refusal.value, "code", None) == 2
