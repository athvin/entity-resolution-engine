"""The S8.1 test-isolation contract: one ephemeral lake namespace per session.

M22 is that "an ephemeral DuckLake per test session" was asserted throughout the
spec and never designed. With one shared catalog schema and one un-namespaced
bucket, test *ordering* becomes load-bearing: a suite that leaves a relation
behind silently feeds it to the next suite, and every snapshot-dependent
assertion is contaminated by whatever ran before it. This module is the design.

A namespace is exactly the pair (``ER_LAKE_METADATA_SCHEMA``,
``ER_LAKE_DATA_PATH``) and **nothing else** in the S4.0b attach sequence varies
between namespaces (S7.2). So the fixtures here set those two variables plus the
alias, and deliberately leave ``ER_CATALOG_DSN`` and every ``ER_S3_*`` variable
exactly as Compose supplied them: a second source of truth for the credentials
would be a second substrate, not a second namespace.

The namespace this session yields is **empty**. S8.1 step 3 runs ``er init``
against it, which does not exist yet (ER-020); creating a relation here would
make "the harness created it" and "the pipeline created it" indistinguishable,
which is precisely what AC4 forbids. ER-019/ER-020 layer the initialised-lake
fixture on top of this one.

Teardown is the reason this lives at session scope rather than in each suite:
:func:`reclaim_namespace` runs S8.1 step 4 in order — expire snapshots, clean up
old files, delete the prefix, ``DETACH``, drop the catalog schema — under a
``try/finally``, so a failing test still reclaims its namespace. A leaked
namespace is not a tidiness problem: it is an orphan Parquet prefix and a
catalog schema that the next session's ``DROP`` will never name.

Session scope also means these fixtures are lazy. The unit layer runs on a bare
runner with no services (S8.1) and requests none of them, so nothing here opens
a connection for a test that does not ask for one.

S8.1's isolation contract has a second half, and it is here too: the session
namespace keeps one *suite* from feeding the next, and :func:`clean_lake` keeps
one *test* from feeding the next. It `DELETE`s from every `ddl.py`-owned relation
and drops every dbt-owned one between tests, with both lists read straight off
the S5 registry -- a hard-coded fourteen would silently rot the first time S5
grows, and the relation set is exactly what "function-isolated" means.
:func:`sub_namespace` is the only sanctioned way to build a *second* universe
inside one session (T-INC-1): two universes sharing a namespace would share
``runs`` and ``run_stages``, which is precisely the comparison that test makes.
``--keep-lake`` suppresses teardown for a human who wants to look at what a red
run left behind, and :func:`pytest_collection_modifyitems` is S8.1's blanket rule
against absolute snapshot versions, enforced at collection so a violating file
names itself rather than failing as an unrelated assertion much later.
"""

from __future__ import annotations

import os
import re
from collections.abc import Callable, Iterator, Mapping
from contextlib import ExitStack
from dataclasses import dataclass
from pathlib import Path
from typing import Final

import duckdb
import pytest
from ulid import ULID

from er.lake.catalog import CatalogConnection, catalog_connect, drop_metadata_schema
from er.lake.ducklake import LAKE_ALIAS, connect, detach
from er.lake.init import init_lake
from er.lake.model import DBT_OWNED, DDL_OWNED, SCHEMA_QUALIFIER
from er.lake.objectstore import ObjectStore

__all__ = [
    "DATA_PATH_TEMPLATE",
    "DELETE_RELATIONS",
    "DROP_RELATIONS",
    "KEEP_LAKE_OPTION",
    "METADATA_SCHEMA_PREFIX",
    "PRESERVED_ENV",
    "RETAINED_MESSAGE",
    "SNAPSHOT_GUARD_HEADER",
    "SUB_NAMESPACE_SUFFIXES",
    "XDIST_WORKER_ENV",
    "IsolationResult",
    "LakeNamespace",
    "SnapshotLiteral",
    "SubNamespace",
    "catalog",
    "clean_lake",
    "compose_env",
    "derive_namespace",
    "er_env",
    "initialised_lake",
    "isolate_namespace",
    "isolation_relations",
    "lake_conn",
    "lake_ns",
    "mint_namespace",
    "namespace_for",
    "object_store",
    "reclaim_namespace",
    "snapshot_literal_violations",
    "sub_namespace",
]

# S8.1 step 2 spells both of these literally, and `er_test_` is also the prefix
# `er lake reset` and every teardown path match on, so it is a constant rather
# than an f-string repeated at each site.
METADATA_SCHEMA_PREFIX: Final[str] = "er_test_"
DATA_PATH_TEMPLATE: Final[str] = "s3://lake/test/{ns}/"

# S8.1 step 1. Integration tests run single-process -- `-n auto` is a unit-layer
# flag -- so this variable is absent in practice; the suffix is implemented
# anyway because the contract names it, and because a future xdist run that
# minted one namespace per worker would otherwise have every worker racing for
# the same schema.
XDIST_WORKER_ENV: Final[str] = "PYTEST_XDIST_WORKER"

# The variables Compose owns (S7.1 `x-er-env`) and the harness must not rewrite.
# Named explicitly rather than matched by prefix so that adding a variable to the
# environment contract is a visible edit here, and so AC2 can assert on a closed
# set rather than on whatever happened to be exported.
PRESERVED_ENV: Final[tuple[str, ...]] = (
    "ER_CATALOG_DSN",
    "ER_S3_ACCESS_KEY_ID",
    "ER_S3_ENDPOINT",
    "ER_S3_REGION",
    "ER_S3_SECRET_ACCESS_KEY",
    "ER_S3_URL_STYLE",
    "ER_S3_USE_SSL",
)

# S8.1 step 4, first two statements, with the one spelling change the pinned
# engine forces. S8.1 writes them catalog-scoped, as
# `CALL lake.expire_snapshots(older_than => now())`; against `duckdb==1.5.5`'s
# ducklake extension that raises
#
#   Catalog Error: Table Function with name expire_snapshots does not exist!
#   Did you mean "main.ducklake_expire_snapshots"?
#
# because DuckDB reads `lake.` there as a *schema* qualifier, not as the attached
# catalog. Only some DuckLake functions are catalog-scoped -- `lake.snapshots()`,
# which `ducklake.py` uses, is one; these two are not, and take the alias as
# their first positional argument instead. The semantics S8.1 specifies, and
# their order, are unchanged. The spec sentence should be corrected to this form.
_EXPIRE_SNAPSHOTS: Final[str] = (
    f"CALL ducklake_expire_snapshots('{LAKE_ALIAS}', older_than => now())"
)
_CLEANUP_OLD_FILES: Final[str] = (
    f"CALL ducklake_cleanup_old_files('{LAKE_ALIAS}', cleanup_all => true)"
)

# The relations function isolation acts on, read off the S5 registry rather than
# transcribed from S8.1's parenthetical fourteen. The parenthetical is a summary of
# the ownership table, not a second declaration of it: a list spelled here would
# agree with S8.1 and disagree with the lake the first time S5 gains a relation, and
# the divergence would surface as one test seeing another test's rows.
DELETE_RELATIONS: Final[tuple[str, ...]] = DDL_OWNED
DROP_RELATIONS: Final[tuple[str, ...]] = DBT_OWNED

# S8.1 names the two universes T-INC-1 builds `er_test_<ns>_a` and `er_test_<ns>_b`.
SUB_NAMESPACE_SUFFIXES: Final[tuple[str, str]] = ("a", "b")

#: Suppresses S8.1 step 4 so a human can inspect what a red run left behind.
KEEP_LAKE_OPTION: Final[str] = "--keep-lake"

#: What ``--keep-lake`` prints instead of reclaiming. The namespace is in it because
#: it is the one value the operator cannot re-derive: it was minted in a process that
#: has by then exited.
RETAINED_MESSAGE: Final[str] = "{option}: retained {schema} at {data_path}"

_LAKE_DATABASE, _LAKE_SCHEMA = SCHEMA_QUALIFIER.split(".", 1)


@dataclass(frozen=True)
class LakeNamespace:
    """The namespace a session owns: S7.2's pair, plus the id both derive from."""

    ns: str
    metadata_schema: str
    data_path: str


def namespace_for(ns: str) -> LakeNamespace:
    """The S7.2 pair ``ns`` implies, per S8.1 step 2.

    One function for the session namespace and for a sub-namespace, so the two can
    never be spelled differently -- a sub-namespace whose `DATA_PATH` did not follow
    the same template would be reclaimed by nothing.
    """
    return LakeNamespace(
        ns=ns,
        metadata_schema=f"{METADATA_SCHEMA_PREFIX}{ns}",
        data_path=DATA_PATH_TEMPLATE.format(ns=ns),
    )


def derive_namespace(identifier: str, worker: str | None) -> str:
    """S8.1 step 1's namespace: ``identifier``, suffixed with the xdist worker.

    Pure, and separate from :func:`mint_namespace`, because the suffix is the only
    half of step 1 that is assertable without minting: one session mints exactly one
    identifier, so "two workers never collide" is unreachable from inside it.

    A blank ``PYTEST_XDIST_WORKER`` counts as absent. xdist exports the variable only
    in a worker process, and an empty value would otherwise yield a namespace ending
    in a bare underscore -- a schema name no `er_test_<ulid>_<worker>` reader would
    parse back into the pair it came from.
    """
    if worker is None or not worker.strip():
        return identifier
    return f"{identifier}_{worker}"


def mint_namespace() -> str:
    """A fresh namespace identifier, per S8.1 step 1.

    Lowercased because it becomes a Postgres schema name: an unquoted identifier
    is folded to lower case by the server, so a mixed-case namespace would be
    created as one string and dropped by another.
    """
    return derive_namespace(str(ULID()).lower(), os.environ.get(XDIST_WORKER_ENV))


def reclaim_namespace(
    connection: duckdb.DuckDBPyConnection,
    store: ObjectStore,
    catalog_connection: CatalogConnection,
    namespace: LakeNamespace,
) -> None:
    """Run S8.1 step 4, in its order, running every step even if one fails.

    The order is normative and not merely tidy: snapshots must be expired and
    unreferenced files cleaned up *before* the prefix is deleted, or DuckLake
    keeps rewriting files into a prefix that is already gone; and the lake must
    be ``DETACH``ed before the catalog schema is dropped, or the drop contends
    with a live attachment.

    ``ExitStack`` rather than five nested ``try/finally`` blocks: callbacks
    unwind last-in-first-out, which is the spec's order read upside down, and
    the stack runs all of them and re-raises rather than abandoning the rest at
    the first failure. A teardown that stopped at a failed ``expire_snapshots``
    would leak both the prefix and the schema -- exactly the contamination M22
    is about.
    """
    with ExitStack() as reclaim:
        reclaim.callback(drop_metadata_schema, catalog_connection, namespace.metadata_schema)
        reclaim.callback(detach, connection)
        reclaim.callback(store.delete_prefix, namespace.data_path)
        reclaim.callback(connection.execute, _CLEANUP_OLD_FILES)
        reclaim.callback(connection.execute, _EXPIRE_SNAPSHOTS)


def pytest_addoption(parser: pytest.Parser) -> None:
    """Register ``--keep-lake``, which suppresses S8.1 step 4."""
    parser.addoption(
        KEEP_LAKE_OPTION,
        action="store_true",
        default=False,
        help=(
            "Do not reclaim the lake namespaces this session mints; print them instead. "
            "For debugging a red run; every retained namespace must then be dropped by hand."
        ),
    )


def _announce(config: pytest.Config, message: str) -> None:
    """Put ``message`` on stdout from a fixture finaliser, even under ``-q``.

    Through the terminal reporter rather than :func:`print`, because a finaliser runs
    inside pytest's capture and a bare ``print`` reaches the operator only when the
    run happens to have been given ``-s``.
    """
    reporter = config.pluginmanager.get_plugin("terminalreporter")
    if reporter is None:
        print(message)
        return
    reporter.write_line(message)


def _reclaim_or_announce(
    config: pytest.Config,
    connection: duckdb.DuckDBPyConnection,
    store: ObjectStore,
    catalog_connection: CatalogConnection,
    namespace: LakeNamespace,
) -> None:
    """S8.1 step 4, unless ``--keep-lake`` asked for the namespace to survive."""
    if config.getoption(KEEP_LAKE_OPTION):
        _announce(
            config,
            RETAINED_MESSAGE.format(
                option=KEEP_LAKE_OPTION,
                schema=namespace.metadata_schema,
                data_path=namespace.data_path,
            ),
        )
        return
    reclaim_namespace(connection, store, catalog_connection, namespace)


def _export(patch: pytest.MonkeyPatch, namespace: LakeNamespace) -> None:
    """S8.1 step 2: the three variables a namespace owns, and no others.

    S7.2 is explicit that nothing else in the S4.0b attach sequence varies between
    namespaces, so ``ER_CATALOG_DSN`` and every ``ER_S3_*`` variable are deliberately
    absent -- a second source of truth for the credentials would be a second
    substrate, not a second namespace.
    """
    patch.setenv("ER_LAKE_METADATA_SCHEMA", namespace.metadata_schema)
    patch.setenv("ER_LAKE_DATA_PATH", namespace.data_path)
    patch.setenv("ER_LAKE_ALIAS", LAKE_ALIAS)


@pytest.fixture(scope="session")
def lake_ns() -> str:
    """The session's namespace identifier (S8.1 step 1)."""
    return mint_namespace()


@pytest.fixture(scope="session")
def compose_env() -> Mapping[str, str]:
    """The Compose-supplied variables, captured before the namespace is exported.

    Requested by :func:`er_env` purely for that ordering: a snapshot taken after
    the export could only prove the variables are *present*, and the claim S8.1
    makes is that they are byte-identical to what Compose set.
    """
    return {name: os.environ[name] for name in PRESERVED_ENV if name in os.environ}


@pytest.fixture(scope="session")
def er_env(lake_ns: str, compose_env: Mapping[str, str]) -> Iterator[LakeNamespace]:
    """Export the namespace (S8.1 step 2), leaving Compose's variables alone.

    A session-scoped fixture cannot request the function-scoped ``monkeypatch``,
    and the namespace has to be in the environment before the first
    ``ducklake.connect()`` renders the S4.0b block, so the context-managed form
    is used directly. Only the three namespace variables are touched: S7.2 is
    explicit that nothing else in the attach sequence varies between namespaces.
    """
    namespace = namespace_for(lake_ns)
    with pytest.MonkeyPatch.context() as patch:
        _export(patch, namespace)
        yield namespace


@pytest.fixture(scope="session")
def object_store(er_env: LakeNamespace) -> ObjectStore:
    """The ER-015 S3 client, built from the environment the namespace exports."""
    return ObjectStore.from_env()


@pytest.fixture(scope="session")
def catalog(er_env: LakeNamespace) -> Iterator[CatalogConnection]:
    """A direct catalog connection, held open for the session.

    Separate from the DuckDB connection by construction (S2.1): teardown drops
    the metadata schema *after* the lake is detached, so the connection that
    issues the drop cannot be one the attachment owns.
    """
    with catalog_connect() as connection:
        yield connection


#: Where :func:`lake_conn` advertises its attachment so :func:`clean_lake` can find
#: it. An autouse fixture that simply *requested* ``lake_conn`` would attach a lake
#: for every unit test as well, on a runner S8.1 says has no services at all; and
#: reaching for it through ``request.getfixturevalue`` at teardown would attach one
#: for a test whose own setup had just failed to. The stash makes "a lake is
#: attached" observable without making it happen.
LAKE_CONNECTION: Final[pytest.StashKey[duckdb.DuckDBPyConnection]] = pytest.StashKey()


@pytest.fixture(scope="session")
def lake_conn(
    er_env: LakeNamespace,
    object_store: ObjectStore,
    catalog: CatalogConnection,
    pytestconfig: pytest.Config,
) -> Iterator[duckdb.DuckDBPyConnection]:
    """The session's attached lake handle, reclaimed on every exit path.

    Depending on ``object_store`` and ``catalog`` is what orders teardown: pytest
    finalises a fixture before the fixtures it requested, so both clients are
    still open when :func:`reclaim_namespace` runs.
    """
    with connect() as connection:
        pytestconfig.stash[LAKE_CONNECTION] = connection
        try:
            yield connection
        finally:
            del pytestconfig.stash[LAKE_CONNECTION]
            _reclaim_or_announce(pytestconfig, connection, object_store, catalog, er_env)


def _relations_in_lake(connection: duckdb.DuckDBPyConnection) -> frozenset[str]:
    """Every relation live in ``lake.main`` right now."""
    rows = connection.execute(
        "SELECT table_name FROM duckdb_tables() WHERE database_name = ? AND schema_name = ?",
        [_LAKE_DATABASE, _LAKE_SCHEMA],
    ).fetchall()
    return frozenset(str(name) for (name,) in rows)


def _drop_every_relation(connection: duckdb.DuckDBPyConnection) -> None:
    """Return the namespace to the relation-free state :func:`lake_conn` yields."""
    for name in sorted(_relations_in_lake(connection)):
        connection.execute(f"DROP TABLE {SCHEMA_QUALIFIER}.{name}")


@dataclass(frozen=True)
class IsolationResult:
    """What one :func:`isolate_namespace` pass actually acted on."""

    deleted: tuple[str, ...]
    dropped: tuple[str, ...]


def isolate_namespace(connection: duckdb.DuckDBPyConnection) -> IsolationResult:
    """S8.1's per-test isolation: empty the `ddl.py`-owned relations, drop the dbt ones.

    The asymmetry is ownership, not taste. `er init` creates the `ddl.py`-owned
    fourteen and S5.1 makes re-creating them a no-op, so emptying them leaves the
    namespace in exactly the state the next test's `er init` would produce -- while
    *dropping* them would make the next test unable to tell `created` from `exists`.
    The dbt-owned relations have the opposite contract: the first `dbt run` of a
    session materializes them under an enforced contract (S5.0), so leaving an
    emptied one behind would hand the next test a relation dbt believes it created.

    Filtered against the live catalog because both halves run on a namespace that may
    hold none of these relations yet: `DELETE` has no `IF EXISTS`, and the very first
    test of a session runs before anything has been created at all.
    """
    live = _relations_in_lake(connection)
    deleted = tuple(name for name in DELETE_RELATIONS if name in live)
    dropped = tuple(name for name in DROP_RELATIONS if name in live)
    for name in deleted:
        connection.execute(f"DELETE FROM {SCHEMA_QUALIFIER}.{name}")
    for name in dropped:
        connection.execute(f"DROP TABLE {SCHEMA_QUALIFIER}.{name}")
    return IsolationResult(deleted=deleted, dropped=dropped)


@pytest.fixture(scope="session", name="derive_namespace")
def derive_namespace_fixture() -> Callable[[str, str | None], str]:
    """:func:`derive_namespace` itself, handed to the layer that tests it.

    A fixture rather than an import, and the same reason applies to
    :func:`isolation_relations` below: three files in this tree are named
    ``conftest``, pytest imports each of them under that one module name, and
    ``import conftest`` therefore resolves to whichever was loaded last. A fixture is
    how a plugin hands a test its own values and cannot resolve to the wrong module.
    """
    return derive_namespace


@pytest.fixture(scope="session")
def isolation_relations() -> tuple[tuple[str, ...], tuple[str, ...]]:
    """What :func:`clean_lake` empties and what it drops, in that order."""
    return DELETE_RELATIONS, DROP_RELATIONS


@pytest.fixture(autouse=True)
def clean_lake(pytestconfig: pytest.Config) -> Iterator[None]:
    """Function isolation (S8.1): no test may inherit another test's rows.

    Autouse, and deliberately a *teardown*: a test that leaves the namespace dirty is
    the one that should pay for it, and cleaning on the way out means the session's
    own teardown, and any test that reads the namespace without asking for isolation,
    both see a namespace in the state `er init` left it.

    It cleans only when a lake is actually attached, which is why it consults
    :data:`LAKE_CONNECTION` rather than requesting ``lake_conn``. The unit layer runs
    on a bare runner (S8.1) and must reach the end of this fixture having opened
    nothing.
    """
    yield
    connection = pytestconfig.stash.get(LAKE_CONNECTION, None)
    if connection is not None:
        isolate_namespace(connection)


@pytest.fixture
def initialised_lake(
    lake_conn: duckdb.DuckDBPyConnection,
) -> Iterator[duckdb.DuckDBPyConnection]:
    """S8.1 step 3, layered onto the session namespace: the lake after `er init`.

    Function-scoped, and it drops what it created. S8.1 puts step 3 in the session
    fixture, but ER-018's own contract is that the namespace it yields holds zero
    relations, and a session-scoped `er init` would make that claim true or false
    depending on which suite happened to run first -- which is the ordering
    dependence M22 exists to remove. Reclaiming here restores the precondition for
    every other suite and leaves ER-018's teardown untouched.

    `er init`'s implementation is called on the session connection rather than
    spawned as a subprocess: a second attachment would create the relations in a
    transaction this connection does not share, so the fixture could yield a handle
    that cannot see them. What it applies is still exactly what the CLI applies --
    there is one :func:`~er.lake.init.init_lake`.
    """
    init_lake(connection=lake_conn)
    try:
        yield lake_conn
    finally:
        _drop_every_relation(lake_conn)


@dataclass(frozen=True)
class SubNamespace:
    """One independent universe: its S7.2 pair and the connection attached to it."""

    namespace: LakeNamespace
    connection: duckdb.DuckDBPyConnection

    @property
    def env(self) -> Mapping[str, str]:
        """The three variables a *subprocess* needs to reach this universe (S7.2).

        Handed out rather than exported, because two universes exist at once and the
        process environment can name only one. A test that drives `er run-all` against
        universe A passes this to the child; the parent's environment keeps pointing
        at the session namespace, which is what stops the arms from interfering.
        """
        return {
            "ER_LAKE_METADATA_SCHEMA": self.namespace.metadata_schema,
            "ER_LAKE_DATA_PATH": self.namespace.data_path,
            "ER_LAKE_ALIAS": LAKE_ALIAS,
        }


@pytest.fixture
def sub_namespace(
    lake_ns: str,
    object_store: ObjectStore,
    catalog: CatalogConnection,
    pytestconfig: pytest.Config,
) -> Iterator[Callable[[str], SubNamespace]]:
    """A factory for S8.1's `er_test_<ns>_a` / `er_test_<ns>_b` universes (T-INC-1).

    Each call repeats S8.1 steps 1-4 under its own suffix: the pair is derived by
    :func:`namespace_for`, exported just long enough for `connect()` to render the
    S4.0b block against it, `er init`-ed, and registered for reclamation. The export
    is scoped to construction on purpose -- leaving it in place would silently
    re-point the *session*, and a later `er init` in the same test would apply itself
    to the wrong lake.

    This is the only sanctioned way to build a second universe. Two universes inside
    one namespace would share `runs` and `run_stages`, so the incremental arm would
    read the full arm's stage rows and G3's comparison would compare a universe with
    itself.

    Teardown is an :class:`~contextlib.ExitStack`, so reclamation is registered before
    `er init` runs and unwinds whether the test passed, failed or raised -- S8.1 makes
    the ``try/finally`` normative because a namespace leaked by a red test is charged
    to the next run's catalog, and a *sub*-namespace leaks two.
    """
    built: dict[str, SubNamespace] = {}
    with ExitStack() as stack:

        def build(suffix: str) -> SubNamespace:
            if suffix in built:
                raise ValueError(
                    f"sub-namespace {suffix!r} already exists in this test; "
                    "two universes may not share one namespace"
                )
            namespace = namespace_for(f"{lake_ns}_{suffix}")
            with pytest.MonkeyPatch.context() as patch:
                _export(patch, namespace)
                connection = stack.enter_context(connect())
                stack.callback(
                    _reclaim_or_announce,
                    pytestconfig,
                    connection,
                    object_store,
                    catalog,
                    namespace,
                )
                init_lake(connection=connection)
            built[suffix] = SubNamespace(namespace=namespace, connection=connection)
            return built[suffix]

        yield build


# --------------------------------------------------------------------------- #
# S8.1's blanket rule, enforced at collection
# --------------------------------------------------------------------------- #

#: What the refusal says. S8.1's remedy is the whole point of the rule, so it is in
#: the message: an absolute version is only ever *right* for the run that produced
#: it, and the id a test needs is on `run_stages` for a named `(run_id, stage)`.
SNAPSHOT_GUARD_HEADER: Final[str] = (
    "S8.1: no test may reference an absolute snapshot version. Capture the id at "
    "runtime from run_stages.snapshot_start / run_stages.snapshot_end for a named "
    "(run_id, stage) and bind it:"
)

# `tests/integration` as *relative* parts rather than as this repository's absolute
# path, so a module collected under any root gets the same answer -- which is what
# lets the guard's own self-test build a violating module under a temporary root
# instead of committing one here, where it would fail this very check.
_INTEGRATION_PARTS: Final[tuple[str, str]] = ("tests", "integration")

# The two shapes S8.1's rule can take in a test. Both require a *digit*: the whole
# point is that a runtime-captured value -- `AT (VERSION => :snap)`, or a comparison
# against a variable -- is the sanctioned form and must collect cleanly.
_SNAPSHOT_RULES: Final[tuple[tuple[str, re.Pattern[str]], ...]] = (
    (
        "absolute version in AT (VERSION => ...)",
        re.compile(r"AT\s*\(\s*VERSION\s*=>\s*\d", re.IGNORECASE),
    ),
    (
        "snapshot_start/snapshot_end against an integer literal",
        re.compile(
            r"snapshot_(?:start|end)\s*(?:[<>!=]=|[<>=])\s*\d"
            r"|\d\s*(?:[<>!=]=|[<>=])\s*snapshot_(?:start|end)",
            re.IGNORECASE,
        ),
    ),
)


@dataclass(frozen=True)
class SnapshotLiteral:
    """One line that pins a snapshot version, located well enough to go and fix."""

    path: Path
    line: int
    rule: str
    text: str

    def __str__(self) -> str:
        return f"{self.path}:{self.line}: {self.rule}: {self.text.strip()}"


def snapshot_literal_violations(source: str, path: Path) -> tuple[SnapshotLiteral, ...]:
    """Every line of ``source`` that names an absolute snapshot version, in file order.

    Line-wise and textual rather than an AST walk, because the rule reaches inside SQL
    string literals, where there is no Python syntax tree to walk.
    """
    found: list[SnapshotLiteral] = []
    for number, line in enumerate(source.splitlines(), start=1):
        for rule, pattern in _SNAPSHOT_RULES:
            if pattern.search(line):
                found.append(SnapshotLiteral(path=path, line=number, rule=rule, text=line))
    return tuple(found)


def _is_integration(path: Path) -> bool:
    """Whether ``path`` lies under a ``tests/integration`` directory."""
    parts = path.parts
    return any(parts[index : index + 2] == _INTEGRATION_PARTS for index in range(len(parts) - 1))


def pytest_collection_modifyitems(items: list[pytest.Item]) -> None:
    """Refuse to collect an integration module that pins a snapshot version (S8.1).

    At collection rather than as a test, because the violation is a property of the
    *source* and not of a run: a test asserting it would have to be requested, would
    report the offence against its own node id, and would say nothing at all about a
    module nobody selected. `pytest.UsageError` is the one refusal that names the
    offence before a single fixture is built, which is what makes the failure the
    file's own rather than the substrate's.
    """
    violations: list[SnapshotLiteral] = []
    scanned: set[Path] = set()
    for item in items:
        path = item.path
        if path in scanned or not _is_integration(path) or not path.is_file():
            continue
        scanned.add(path)
        violations.extend(snapshot_literal_violations(path.read_text(encoding="utf-8"), path))
    if violations:
        raise pytest.UsageError("\n".join([SNAPSHOT_GUARD_HEADER, *map(str, violations)]))
