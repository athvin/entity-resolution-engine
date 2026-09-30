"""Integration-layer fixtures (DesignDoc.md S8.1), and the T-INV-1 finalizer (S8.3).

The session harness — `lake_ns`, `er_env`, `object_store`, `catalog`, `lake_conn` —
lives one directory up in `tests/conftest.py`, because the namespace contract is a
property of the *session* and not of this layer, and because the unit layer must be able
to import the same module on a bare runner without requesting any of it.

**T-INV-1 is armed here.** S8.3 makes it an autouse session-and-function finalizer that
runs after **every** integration scenario: it is the suite's only standing invariant, and
the only guard against the DuckDB label-propagation path and Splink's
`cluster_pairwise_predictions_at_threshold` drifting apart. The clauses themselves live
in `tests/helpers/invariants.py` so that this finalizer and the explicit node in
`test_invariants.py` assert the identical thing.

Two details of the wiring are load-bearing.

**Failures are reported under T-INV-1's own node id.** S8.3: "A scenario test that passes
while T-INV-1 fails is reported as a T-INV-1 failure against that scenario's node id."
A bare `raise` in a fixture finalizer produces a teardown error attributed to whichever
scenario happened to run last, which tells a reader that the scenario is broken when the
invariant is. So the failure is *also* emitted as a synthetic report under
`tests/integration/test_invariants.py::test_membership_equals_connected_components`,
which is the node id S8.3 names and which appears in the junit XML CI reads.

**The finalizer runs before the namespace is cleaned.** `clean_lake` in the parent
conftest is autouse too and empties the namespace on teardown. Fixtures finalise in
reverse setup order and a parent conftest's fixture is set up first, so it tears down
last — this one therefore sees the state the scenario left, which is the only state
worth asserting against. That ordering is a consequence of where the two fixtures are
defined rather than of anything declared here, so it is stated rather than assumed.
"""

from __future__ import annotations

from collections.abc import Iterator
from typing import Final

import duckdb
import pytest
from _pytest.reports import TestReport
from helpers.invariants import assert_membership_equals_components

from er.matching.api import SPLINK_RELATION_PREFIX

#: Modules whose subject is the HARNESS rather than the pipeline, and which T-INV-1
#: therefore does not run after.
#:
#: S8.3 scopes the finalizer to "every integration **scenario**", and S8.2.1 defines a
#: scenario as a fixture directory the pipeline is run over. `test_harness_isolation`
#: is not one: `test_every_ddl_owned_relation_receives_a_row` writes one synthetic row
#: into every `ddl.py`-owned relation — every VARCHAR column set to `'x'` — precisely to
#: prove the isolation fixture removes them again. At the moment that test ends it
#: therefore holds, by construction, an `entity_membership` row pointing at an entity
#: whose status is `'x'`, which is exactly what clause 2 forbids of pipeline output.
#:
#: The exemption is a named list rather than a pattern so that adding to it is a
#: deliberate act with a reason attached. A new module tripping T-INV-1 is a signal to
#: look at that module, not to widen this set.
HARNESS_MODULES: Final[frozenset[str]] = frozenset({"test_harness_isolation"})

#: The S8.3 node id every T-INV-1 failure is reported under, and its parts.
T_INV_1_FILE: Final = "tests/integration/test_invariants.py"
T_INV_1_NAME: Final = "test_membership_equals_connected_components"
T_INV_1_NODEID: Final = f"{T_INV_1_FILE}::{T_INV_1_NAME}"


def _lake_connection(config: pytest.Config) -> duckdb.DuckDBPyConnection | None:
    """The session's lake connection, or ``None`` when no lake was ever attached.

    The key is found through the plugin manager rather than imported. `tests/` has no
    `__init__.py`, so `tests/conftest.py` and this file are both importable as the
    module name ``conftest`` — the ambiguity the parent conftest documents where it
    exposes `derive_namespace` as a fixture instead of a symbol. Asking the plugin
    manager for the module pytest actually loaded sidesteps the question entirely, and
    it consults the stash rather than requesting `lake_conn`, for the reason
    :data:`LAKE_CONNECTION` states: requesting it would ATTACH a lake for a test whose
    own setup had just declined to.
    """
    for plugin in config.pluginmanager.get_plugins():
        key = getattr(plugin, "LAKE_CONNECTION", None)
        if isinstance(key, pytest.StashKey):
            found: duckdb.DuckDBPyConnection | None = config.stash.get(key, None)
            return found
    return None


def _emit_t_inv_1_failure(config: pytest.Config, detail: str) -> None:
    """Record a failed T-INV-1 testcase in the run's report stream.

    Constructed as a report rather than raised as a test, because the invariant is a
    finalizer: there is no test being executed to fail. Emitting it through
    ``pytest_runtest_logreport`` is what puts it in the junit XML under the node id
    S8.3 names, which is what AC6 verifies.
    """
    report = TestReport(
        nodeid=T_INV_1_NODEID,
        location=(T_INV_1_FILE, None, T_INV_1_NAME),
        keywords={T_INV_1_NAME: 1},
        outcome="failed",
        longrepr=detail,
        when="call",
    )
    config.hook.pytest_runtest_logreport(report=report)


def _check_invariant(config: pytest.Config, scope: str) -> None:
    """Run T-INV-1 and, on failure, report it under its own node id before re-raising.

    The re-raise is deliberate and the DoD requires it: a finalizer that reported the
    failure and swallowed it would leave the suite green, and an invariant nobody fails
    on is documentation.
    """
    connection = _lake_connection(config)
    if connection is None:
        return
    try:
        assert_membership_equals_components(connection)
    except AssertionError as failure:
        _emit_t_inv_1_failure(config, f"T-INV-1 failed after the {scope} scope:\n{failure}")
        raise


#: Splink scratch lives in the primary (`:memory:`) database of the session connection,
#: which the S8.1 function-isolation fixture does not touch: that fixture's subject is
#: the lake, and it `DELETE`s `ddl.py`-owned relations and drops dbt-owned ones. Nothing
#: cleans the memory database, so a test that builds a Splink api and never releases it
#: hands its `__splink__` relations to every later test in the session.
#:
#: That is not a theoretical hazard. `test_full_match::test_no_splink_relations_in_lake`
#: counts scratch across the whole primary database, so it failed with 16 leaked
#: relations whenever T-BLK-1 ran before it -- and CI never saw it, because the workflow
#: shards integration across 32 jobs and the two modules never share a process. The
#: failure was reported against the module that *observed* the leak rather than the one
#: that caused it, which is the expensive half of the bug.
#:
#: So the leak is detected where it happens. The check runs on the session connection
#: only; `sub_namespace` universes own their connections and discard them at teardown,
#: and `test_splink_isolation` deliberately leaves scratch on those.
#: A leak is as real through a view as through a table, and `duckdb_tables()` does not
#: report views -- the same reason `leaked_splink_relations` queries both catalogs. The
#: kind is selected alongside the name because DuckDB refuses `DROP VIEW` on a table.
_SCRATCH_SQL: Final[str] = """
SELECT 'TABLE', schema_name, table_name FROM duckdb_tables()
 WHERE database_name = current_database() AND table_name LIKE ?
UNION ALL
SELECT 'VIEW', schema_name, view_name FROM duckdb_views()
 WHERE database_name = current_database() AND view_name LIKE ?
"""


def _sweep_splink_scratch(connection: duckdb.DuckDBPyConnection) -> tuple[str, ...]:
    """Drop every `__splink__` relation in the primary database; name what was there.

    Cleaning as well as reporting is deliberate. An assertion that only raised would
    leave the scratch in place, so every subsequent test in the session would fail the
    same way and the run would say nothing about which one leaked. Sweeping means
    exactly one test fails: the one that left something behind.
    """
    pattern = f"{SPLINK_RELATION_PREFIX}%"
    rows = connection.execute(_SCRATCH_SQL, [pattern, pattern]).fetchall()
    found = sorted((str(kind), str(schema), str(name)) for kind, schema, name in rows)
    for kind, schema, name in found:
        connection.execute(f'DROP {kind} IF EXISTS "{schema}"."{name}"')
    return tuple(f"{schema}.{name}" for _, schema, name in found)


@pytest.fixture(autouse=True)
def no_splink_scratch_after_each_test(pytestconfig: pytest.Config) -> Iterator[None]:
    """Fail the test that leaves Splink scratch in the session's memory database.

    Every `splink_api` in `src/er` is paired with a `cleanup_splink`; this holds test
    helpers to the same rule, and attributes a breach to the test that committed it
    rather than to whichever later module happens to count scratch.
    """
    yield
    # Through the plugin manager, for the reason :func:`_lake_connection` gives: three
    # files in this tree are importable as `conftest`, and requesting `lake_conn` would
    # ATTACH a lake for a test whose own setup declined to.
    connection = _lake_connection(pytestconfig)
    if connection is None:
        return
    leaked = _sweep_splink_scratch(connection)
    assert not leaked, (
        f"{len(leaked)} Splink scratch relation(s) survived this test in the session's "
        f"primary database: {', '.join(leaked)}. Every splink_api() needs a paired "
        f"cleanup_splink() in a finally -- nothing else cleans the memory database, so "
        f"these would have been inherited by every later test in this session."
    )


@pytest.fixture(autouse=True)
def t_inv_1_after_each_test(
    request: pytest.FixtureRequest, pytestconfig: pytest.Config
) -> Iterator[None]:
    """T-INV-1 after every integration scenario (S8.3).

    Skipped for :data:`HARNESS_MODULES`, whose subject is the isolation harness itself
    and which deliberately leave rows no pipeline would write.
    """
    yield
    if request.module.__name__ in HARNESS_MODULES:
        return
    _check_invariant(pytestconfig, "function")


@pytest.fixture(scope="session", autouse=True)
def t_inv_1_after_session(pytestconfig: pytest.Config) -> Iterator[None]:
    """T-INV-1 once more when the session ends, over whatever state survived it."""
    yield
    _check_invariant(pytestconfig, "session")
