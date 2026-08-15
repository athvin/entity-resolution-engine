"""`er init` and `er lake reset` (DesignDoc.md S4.0, S4.0b, S5.1, S8.1).

M19 is that nothing anywhere invoked `ddl.py`: the registry existed, the apply
function existed, and no verb called either, so M1's exit criterion — a lake with
the fourteen `ddl.py`-owned relations in it — was unreachable. This module is the
call site, and it is the only one: `er init` is "the only thing that ever creates
`ddl.py`-owned tables" (S7.1).

Three rules govern it, and each is a decision the spec makes rather than a choice
made here:

* **`DATA_PATH` is immutable.** It is written into the catalog on first attach
  (S4.0b) and a later disagreement with ``$ER_LAKE_DATA_PATH`` is a hard exit ``3``
  carrying :data:`DATA_PATH_MISMATCH` verbatim — never a silent re-point, because a
  re-pointed lake reads back as an empty one and looks like data loss. The check
  runs on the *catalog* connection, before the attach, so the mismatching value
  never reaches DuckDB.
* **`er init` emits no DDL of its own.** Every statement comes from
  :mod:`er.lake.ddl`, which owns the ownership guard; this module decides *when*
  and reports *what happened*. That is also why ``--force`` recreates nothing: a
  destructive recreate would have to emit a ``DROP`` from here, and S5.0's split
  puts every statement against a `ddl.py`-owned relation in one module.
* **`er lake reset` is a writer like any other.** It takes the same S4.0b tenant
  advisory lock every writer takes and exits ``3`` when it cannot, and it is
  reachable only through ``--confirm-tenant`` matching the validated config (exit
  ``2``), so a typo can never reach the destructive path.

Out of scope by construction: the `runs` / `run_stages` rows both verbs will
persist (ER-023), and generalising the advisory lock onto every mutating command
(ER-024). `er init` therefore takes no lock yet — S4.0 calls it idempotent and
single-writer, and ER-024 is where "every writer" becomes one mechanism.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Final

import duckdb

from er.errors import ConfigError, PreconditionFailure
from er.lake.catalog import (
    advisory_lock,
    catalog_connect,
    drop_metadata_schema,
    read_data_path,
)
from er.lake.ddl import RelationResult, SchemaBreakingError, apply, diff, evolve
from er.lake.ducklake import connect
from er.lake.env import require_env
from er.lake.objectstore import ObjectStore

__all__ = [
    "DATA_PATH_MISMATCH",
    "UNKNOWN_TENANT",
    "DataPathMismatchError",
    "ResetResult",
    "TenantMismatchError",
    "init_lake",
    "reset_lake",
]

#: S4.0's message for the immutability failure, character for character including
#: the remedy clause. Operators grep for it and `tests/integration/test_init.py`
#: matches it exactly, so it is spelled once and formatted, never rebuilt.
DATA_PATH_MISMATCH: Final[str] = (
    "lake DATA_PATH immutable: catalog={catalog} env={env}; "
    "use 'er lake reset --confirm-tenant {tenant}' to destroy and recreate this namespace"
)

#: What the remedy clause says when the tenant is unknown. S4.0's required env for
#: `er init` names lake variables only, so the command runs without an S6 document
#: and there is then no tenant to name; the placeholder keeps the message the shape
#: S4.0 documents rather than emitting an empty `--confirm-tenant`, which would be a
#: command that silently does nothing if pasted.
UNKNOWN_TENANT: Final[str] = "<tenant>"


class DataPathMismatchError(PreconditionFailure):
    """The catalog's recorded `DATA_PATH` is not ``$ER_LAKE_DATA_PATH`` (S4.0, S7.2).

    A :class:`~er.errors.PreconditionFailure`, so exit ``3`` and the ``precondition``
    class both follow from the S4.7 table rather than being chosen here.
    """

    def __init__(self, *, catalog: str, env: str, tenant: str | None) -> None:
        super().__init__(
            DATA_PATH_MISMATCH.format(
                catalog=catalog,
                env=env,
                tenant=tenant if tenant is not None else UNKNOWN_TENANT,
            )
        )
        self.catalog = catalog
        self.env = env


class TenantMismatchError(ConfigError):
    """``--confirm-tenant`` is not the `tenant` of the validated config (S4.0).

    Exit ``2``, and raised before anything is opened, listed or dropped: the guard
    exists so that the destructive path "cannot be reached by a typo", which is only
    true while the comparison is the first thing :func:`reset_lake` does.
    """

    def __init__(self, *, tenant: str | None, confirm_tenant: str) -> None:
        super().__init__(f"tenant mismatch: --confirm-tenant {confirm_tenant!r} is not {tenant!r}")
        self.tenant = tenant
        self.confirm_tenant = confirm_tenant


@dataclass(frozen=True, slots=True)
class ResetResult:
    """What `er lake reset` destroyed, in the terms S4.0's stdout column names.

    ``dropped_schema`` is ``None`` when there was no schema to drop — reset is a
    teardown path and runs after a failure that never created one — so a caller can
    tell "destroyed" from "was already gone" without a second query.
    """

    dropped_schema: str | None
    deleted_prefix: str
    objects_deleted: int


def _applied(connection: duckdb.DuckDBPyConnection) -> tuple[RelationResult, ...]:
    """Create the missing relations and reconcile the live ones (S5.1).

    Differences are classified *before* the first statement runs. S5.1 requires that
    a lake carrying a breaking difference is left exactly as it was found and that no
    snapshot is committed; :func:`~er.lake.ddl.evolve` guarantees that for the
    columns it adds, but :func:`~er.lake.ddl.apply` would already have created the
    absent relations by the time it looked.
    """
    try:
        for difference in diff(connection):
            if difference.breaking:
                raise SchemaBreakingError(difference)
        results = apply(connection)
        evolve(connection)
    except SchemaBreakingError as exc:
        # `ddl.py` raises outside the `er.errors` hierarchy on purpose (it is reached
        # from preflights that have no `run_stages` row yet), and the CLI classifies
        # by exception type. Rethrown with the S4.7 class attached and the S5.1
        # wording untouched -- the message is the operator's grep token.
        raise PreconditionFailure(str(exc)) from exc
    return results


def init_lake(
    *,
    tenant: str | None = None,
    connection: duckdb.DuckDBPyConnection | None = None,
) -> tuple[RelationResult, ...]:
    """Apply the `ddl.py`-owned registry to the namespace in the environment.

    Idempotent (S5.1): a second call reports ``exists`` for every relation and
    executes nothing.

    Args:
        tenant: the `tenant` of the validated config, for the remedy clause of the
            immutability message. Absent when the command ran without an S6 document.
        connection: an already-attached lake connection to apply against. The S8.1
            harness passes its session connection, because a second attachment would
            create the relations in a transaction the session's connection does not
            share. Left ``None``, this opens and detaches its own (S4.0: attach,
            apply, detach).

    Raises:
        DataPathMismatchError: the catalog records a different `DATA_PATH` (exit 3).
        er.errors.PreconditionFailure: a breaking schema difference (exit 3, S5.1).
    """
    schema = require_env("ER_LAKE_METADATA_SCHEMA")
    data_path = require_env("ER_LAKE_DATA_PATH")

    # On the catalog connection and before the attach: an `ATTACH` carrying the
    # mismatching DATA_PATH is exactly the silent re-point S4.0b forbids, so the
    # value must be compared while DuckDB has still never seen it.
    with catalog_connect() as catalog_connection:
        recorded = read_data_path(catalog_connection, schema)
    if recorded is not None and recorded != data_path:
        raise DataPathMismatchError(catalog=recorded, env=data_path, tenant=tenant)

    if connection is not None:
        return _applied(connection)
    with connect() as opened:
        return _applied(opened)


def reset_lake(*, tenant: str | None, confirm_tenant: str) -> ResetResult:
    """Destroy the namespace: drop the metadata schema, delete the prefix (S4.0).

    The lake is never attached. Reset removes the catalog schema that *is* the
    DuckLake metadata, so an attachment held across it would be reading a catalog
    that no longer exists; and the prefix delete is an S3 operation DuckDB's httpfs
    cannot perform at all (S2.1), which is why it goes through
    :class:`~er.lake.objectstore.ObjectStore` and its root guard.

    Raises:
        TenantMismatchError: ``confirm_tenant`` is not ``tenant`` (exit 2).
        er.errors.PreconditionFailure: the tenant lock is held (exit 3, S4.0b).
    """
    if tenant is None or confirm_tenant != tenant:
        raise TenantMismatchError(tenant=tenant, confirm_tenant=confirm_tenant)

    schema = require_env("ER_LAKE_METADATA_SCHEMA")
    data_path = require_env("ER_LAKE_DATA_PATH")
    # Built before the lock is taken: an S3 variable that is absent or malformed is
    # an exit-2 config fault, and discovering it while holding the writer lock would
    # mean the schema is already gone before the prefix can be reached.
    store = ObjectStore.from_env()

    # The lock's own connection issues the drop. It is a catalog connection like any
    # other, and a second one would be a second session that the lock does not cover
    # -- the drop has to happen inside the critical section, not merely after it.
    with advisory_lock(tenant) as catalog_connection:
        dropped = drop_metadata_schema(catalog_connection, schema)
        deleted = store.delete_prefix(data_path)
    return ResetResult(
        dropped_schema=schema if dropped else None,
        deleted_prefix=data_path,
        objects_deleted=deleted,
    )
