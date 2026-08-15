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
"""

from __future__ import annotations

import os
from collections.abc import Iterator, Mapping
from contextlib import ExitStack
from dataclasses import dataclass
from typing import Final

import duckdb
import pytest
from ulid import ULID

from er.lake.catalog import CatalogConnection, catalog_connect, drop_metadata_schema
from er.lake.ducklake import LAKE_ALIAS, connect, detach
from er.lake.objectstore import ObjectStore

__all__ = [
    "DATA_PATH_TEMPLATE",
    "METADATA_SCHEMA_PREFIX",
    "PRESERVED_ENV",
    "XDIST_WORKER_ENV",
    "LakeNamespace",
    "catalog",
    "compose_env",
    "er_env",
    "lake_conn",
    "lake_ns",
    "mint_namespace",
    "object_store",
    "reclaim_namespace",
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


@dataclass(frozen=True)
class LakeNamespace:
    """The namespace a session owns: S7.2's pair, plus the id both derive from."""

    ns: str
    metadata_schema: str
    data_path: str


def mint_namespace() -> str:
    """A fresh namespace identifier, per S8.1 step 1.

    Lowercased because it becomes a Postgres schema name: an unquoted identifier
    is folded to lower case by the server, so a mixed-case namespace would be
    created as one string and dropped by another.
    """
    identifier = str(ULID()).lower()
    worker = os.environ.get(XDIST_WORKER_ENV)
    if worker is None or not worker.strip():
        return identifier
    return f"{identifier}_{worker}"


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
    namespace = LakeNamespace(
        ns=lake_ns,
        metadata_schema=f"{METADATA_SCHEMA_PREFIX}{lake_ns}",
        data_path=DATA_PATH_TEMPLATE.format(ns=lake_ns),
    )
    with pytest.MonkeyPatch.context() as patch:
        patch.setenv("ER_LAKE_METADATA_SCHEMA", namespace.metadata_schema)
        patch.setenv("ER_LAKE_DATA_PATH", namespace.data_path)
        patch.setenv("ER_LAKE_ALIAS", LAKE_ALIAS)
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


@pytest.fixture(scope="session")
def lake_conn(
    er_env: LakeNamespace, object_store: ObjectStore, catalog: CatalogConnection
) -> Iterator[duckdb.DuckDBPyConnection]:
    """The session's attached lake handle, reclaimed on every exit path.

    Depending on ``object_store`` and ``catalog`` is what orders teardown: pytest
    finalises a fixture before the fixtures it requested, so both clients are
    still open when :func:`reclaim_namespace` runs.
    """
    with connect() as connection:
        try:
            yield connection
        finally:
            reclaim_namespace(connection, object_store, catalog, er_env)
