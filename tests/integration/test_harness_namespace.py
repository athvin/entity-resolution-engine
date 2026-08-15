"""The S8.1 session harness, against the real Compose substrate.

Three of the six acceptance criteria are observable from inside one session: the
environment the harness exports, the schema it attaches, and the fact that it
attaches an *empty* one. The other three are not. "Teardown reclaimed the
namespace" cannot be asserted by the session whose teardown is being tested,
because that session is still running when its assertions execute and is gone
when its teardown finishes; and one session mints exactly one namespace, so
neither the `PYTEST_XDIST_WORKER` suffix nor "two sessions differ" is reachable
from within it.

So those three drive a **nested pytest session** in a subprocess, taking its
namespace from the same `tests/conftest.py` harness. The outer session then
asserts, from its own catalog connection and its own S3 client, what the inner
one left behind. The failing variant is the one that matters: S8.1 makes the
`try/finally` normative precisely because a suite that reclaimed its namespace
only on the happy path would leak one on every red build.

The nested session is handed the harness with `-p conftest` over a `PYTHONPATH`
carrying `tests/`, rather than a copy: a copy would test a copy. For the same
reason this module imports **nothing** from the harness and spells S8.1's
`er_test_<ns>`, `s3://lake/test/<ns>/` and the S7.1 variable list out as
literals -- a test that reused the implementation's own constants could not
catch a wrong constant.

Nothing here asserts a snapshot count or an absolute snapshot version (S8.1, S4
preamble); the empty-namespace check counts *relations*, which is a different
claim and is the one AC4 makes.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from collections.abc import Mapping
from pathlib import Path

import duckdb
import pytest

from er.lake.catalog import CatalogConnection, metadata_schema_exists
from er.lake.ducklake import LAKE_ALIAS
from er.lake.objectstore import ObjectStore

# Crockford base32, lowercased: the alphabet drops `i`, `l`, `o` and `u` so a
# namespace cannot be transcribed into a different one, and 26 characters is the
# ULID encoding's fixed width.
NS_PATTERN = re.compile(r"^[0-9a-hjkmnp-tv-z]{26}$")

# S7.1 `x-er-env`, minus the three the harness owns. Compose supplies all seven,
# so a missing one means the harness dropped it rather than that the substrate
# never had it.
COMPOSE_OWNED_ENV = (
    "ER_CATALOG_DSN",
    "ER_S3_ACCESS_KEY_ID",
    "ER_S3_ENDPOINT",
    "ER_S3_REGION",
    "ER_S3_SECRET_ACCESS_KEY",
    "ER_S3_URL_STYLE",
    "ER_S3_USE_SSL",
)

XDIST_WORKER_ENV = "PYTEST_XDIST_WORKER"

# Where the harness lives, put on the nested session's `PYTHONPATH` so
# `-p conftest` resolves to the real module.
TESTS_DIR = Path(__file__).resolve().parents[1]

# How a nested session hands its namespace back. A file rather than stdout
# parsing: `-q` output is pytest's format to change, and the namespace is the one
# value the outer assertions cannot re-derive.
NS_HANDBACK_ENV = "ER_HARNESS_NS_FILE"

# Requests `lake_ns` and nothing else, so it mints an identifier without
# attaching anything -- which is what makes the AC1 probe cheap.
NESTED_MINTS_ONLY = '''\
"""A nested session that reports the namespace the harness minted for it."""

import os
from pathlib import Path


def test_reports_its_namespace(lake_ns):
    Path(os.environ["{handback}"]).write_text(lake_ns, encoding="utf-8")
'''

# Writes both a relation and an object, because they are reclaimed by different
# steps of S8.1 step 4 -- the table's Parquet by `cleanup_old_files` plus the
# prefix delete, the catalog rows by the `DROP SCHEMA`.
_NESTED_FILLS = '''\
"""A nested session that takes a namespace from the S8.1 harness and fills it."""

import os
from pathlib import Path

PROBE_TABLE = "lake.main.harness_probe"


def _fill(lake_ns, er_env, lake_conn, object_store):
    Path(os.environ["{handback}"]).write_text(lake_ns, encoding="utf-8")
    lake_conn.execute(f"CREATE TABLE {{PROBE_TABLE}} (n INTEGER)")
    lake_conn.execute(f"INSERT INTO {{PROBE_TABLE}} VALUES (1)")
    object_store.put_bytes(er_env.data_path + "probe.txt", b"probe")
    assert object_store.list_prefix(er_env.data_path), (
        "the nested session wrote nothing, so teardown would have nothing to reclaim"
    )
'''

NESTED_PASSES = (
    _NESTED_FILLS
    + """

def test_fills_the_namespace(lake_ns, er_env, lake_conn, object_store):
    _fill(lake_ns, er_env, lake_conn, object_store)
"""
)

NESTED_FAILS = (
    _NESTED_FILLS
    + """

def test_fills_the_namespace_then_raises(lake_ns, er_env, lake_conn, object_store):
    _fill(lake_ns, er_env, lake_conn, object_store)
    raise AssertionError("deliberate failure: teardown must still reclaim the namespace")
"""
)


def _run_nested_session(
    tmp_path: Path, body: str, extra_env: Mapping[str, str] | None = None
) -> tuple[subprocess.CompletedProcess[str], str]:
    """Run `body` as a one-test pytest session; return it with the namespace it minted."""
    module = tmp_path / "test_nested_namespace.py"
    module.write_text(body.format(handback=NS_HANDBACK_ENV), encoding="utf-8")
    handback = tmp_path / "namespace.txt"
    # Removed rather than overwritten: a second nested run in the same `tmp_path`
    # that died before reaching the harness would otherwise hand back the first
    # run's namespace, and the assertions would pass against the wrong one.
    handback.unlink(missing_ok=True)

    # The nested run inherits the *outer* namespace in its environment; its own
    # `er_env` overwrites both variables before anything connects, which is what
    # makes the two sessions independent rather than nested in one namespace.
    environment = dict(os.environ)
    environment[NS_HANDBACK_ENV] = str(handback)
    environment["PYTHONPATH"] = os.pathsep.join(
        [str(TESTS_DIR), *([environment["PYTHONPATH"]] if environment.get("PYTHONPATH") else [])]
    )
    environment.update(extra_env or {})

    completed = subprocess.run(
        [sys.executable, "-m", "pytest", str(module), "-q", "-p", "conftest"],
        # From `tmp_path`, so the nested rootdir holds no `conftest.py` of its own
        # and `-p conftest` cannot resolve to anything but the harness.
        cwd=tmp_path,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    assert handback.is_file(), (
        "the nested session never reached the harness:\n" + completed.stdout + completed.stderr
    )
    return completed, handback.read_text(encoding="utf-8").strip()


def _assert_namespace_reclaimed(
    store: ObjectStore, catalog_connection: CatalogConnection, ns: str
) -> None:
    prefix = f"s3://lake/test/{ns}/"
    keys = store.list_prefix(prefix)
    assert keys == [], f"teardown left {len(keys)} object(s) under {prefix}: {keys[:5]}"
    schema = f"er_test_{ns}"
    assert not metadata_schema_exists(catalog_connection, schema), (
        f"teardown left the catalog schema {schema} behind"
    )


def test_ns_shape_and_xdist_suffix(tmp_path: Path, lake_ns: str) -> None:
    # Integration tests run single-process (S8.1), so this session's own
    # namespace is the unsuffixed form.
    assert NS_PATTERN.match(lake_ns), f"{lake_ns!r} is not a lowercased Crockford ULID"

    completed, plain_ns = _run_nested_session(tmp_path, NESTED_MINTS_ONLY)
    assert completed.returncode == 0, completed.stdout + completed.stderr
    assert NS_PATTERN.match(plain_ns), f"{plain_ns!r} is not a lowercased Crockford ULID"
    assert plain_ns != lake_ns, "two sessions minted the same namespace"

    suffixed_run, suffixed_ns = _run_nested_session(
        tmp_path, NESTED_MINTS_ONLY, {XDIST_WORKER_ENV: "gw3"}
    )
    assert suffixed_run.returncode == 0, suffixed_run.stdout + suffixed_run.stderr
    identifier, _, worker = suffixed_ns.partition("_")
    assert worker == "gw3"
    assert NS_PATTERN.match(identifier)
    assert suffixed_ns == f"{identifier}_gw3"
    assert identifier != plain_ns


@pytest.mark.usefixtures("er_env")
def test_env_exports_namespace_and_preserves_compose_vars(
    lake_ns: str, compose_env: Mapping[str, str]
) -> None:
    assert os.environ["ER_LAKE_METADATA_SCHEMA"] == f"er_test_{lake_ns}"
    assert os.environ["ER_LAKE_DATA_PATH"] == f"s3://lake/test/{lake_ns}/"
    assert os.environ["ER_LAKE_ALIAS"] == LAKE_ALIAS == "lake"

    assert set(compose_env) == set(COMPOSE_OWNED_ENV)
    for name, captured in compose_env.items():
        assert os.environ[name] == captured, f"{name} was rewritten by the harness"


def test_lake_conn_attaches_namespaced_schema(
    lake_conn: duckdb.DuckDBPyConnection, catalog: CatalogConnection, lake_ns: str
) -> None:
    # Succeeding is the assertion; the rows deliberately are not, because no test
    # may assert how many snapshots a namespace holds.
    snapshots = lake_conn.execute(f"SELECT * FROM {LAKE_ALIAS}.snapshots()")
    assert snapshots.description is not None
    assert "snapshot_id" in [column[0] for column in snapshots.description]

    assert metadata_schema_exists(catalog, f"er_test_{lake_ns}")


def test_namespace_starts_empty_of_relations(lake_conn: duckdb.DuckDBPyConnection) -> None:
    row = lake_conn.execute(
        "SELECT count(*) FROM duckdb_tables() WHERE database_name = ?", [LAKE_ALIAS]
    ).fetchone()
    assert row is not None
    assert row[0] == 0, (
        "the harness created a relation; the namespace it yields is empty and "
        "`er init` is wired in by ER-020"
    )


def test_teardown_drops_schema_and_prefix(
    tmp_path: Path, object_store: ObjectStore, catalog: CatalogConnection, lake_ns: str
) -> None:
    completed, nested_ns = _run_nested_session(tmp_path, NESTED_PASSES)

    assert completed.returncode == 0, completed.stdout + completed.stderr
    assert nested_ns != lake_ns, "the nested session reused the outer session's namespace"
    _assert_namespace_reclaimed(object_store, catalog, nested_ns)


def test_teardown_runs_on_test_failure(
    tmp_path: Path, object_store: ObjectStore, catalog: CatalogConnection, lake_ns: str
) -> None:
    completed, nested_ns = _run_nested_session(tmp_path, NESTED_FAILS)

    assert completed.returncode != 0, "the deliberately failing nested session reported success"
    assert nested_ns != lake_ns
    _assert_namespace_reclaimed(object_store, catalog, nested_ns)
