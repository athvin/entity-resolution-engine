"""Tenant provisioning: a dedicated catalog database per org, plus its lake.

docs/backend-design.md §7's lifecycle, automated: ``POST /v1/orgs`` derives the
tenant's namespace, seeds its config from the reference template, and enqueues
a ``provision`` job; the job — running in the runner subprocess with the org's
``ER_*`` environment already injected — creates the org's **dedicated Postgres
database** (hard isolation: no tenant's tables share a database with another's)
and runs the engine's ``er init`` against it.

The split follows the API's own contract: the handler only writes rows and
returns; everything slow or failure-prone (``CREATE DATABASE``, the DuckLake
attach, S3) happens in the job, where the existing retry matrix already knows
what to do with each exit code.

Every step is idempotent — derivation is deterministic, ``CREATE DATABASE`` is
guarded by a ``pg_database`` probe, config seeding replays as a no-op, and
``init_lake`` reports ``exists`` on a re-run — so a retried job or a replayed
``POST`` resumes onboarding instead of corrupting it.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import duckdb
import psycopg
from psycopg import errors as pg_errors
from psycopg import sql
from psycopg.rows import dict_row

from er.config.loader import ConfigValidationError
from er.errors import ErError, classify, exit_code_for
from er.lake.env import EnvError
from er.service import RunOutcome, StageOutcome
from erserver.settings import ServerSettings

__all__ = [
    "ProvisioningNotConfigured",
    "TenantPlan",
    "create_tenant_database",
    "execute",
    "plan_for",
    "render_seed_config",
    "seed_config",
    "seed_template_text",
    "tenant_db_name",
    "tenant_namespace",
]

#: The env var the runner reads for the CREATEDB-capable DSN. Inherited from
#: the dispatcher's process environment — a DSN never rides in job rows.
MAINT_DSN_VAR = "ERSERVER_MAINT_DSN"


class ProvisioningNotConfigured(RuntimeError):
    """A required provisioning setting is unset or invalid; the message names it."""


def tenant_namespace(org: str) -> str:
    """The org's deterministic namespace: metadata schema, config tenant, lock key.

    ``t_<org>_<hash>``: safe as an unquoted Postgres identifier (orgs are
    already ``[a-z0-9][a-z0-9_-]*``), truncation-proof at 51 chars, and the
    8-hex-char blake2b suffix keeps orgs distinct that normalize identically
    (``a-b`` vs ``a_b``).
    """
    digest = hashlib.blake2b(org.encode(), digest_size=4).hexdigest()
    return f"t_{org.replace('-', '_')[:40]}_{digest}"


def tenant_db_name(org: str) -> str:
    """The org's dedicated catalog database — under Postgres's 63-byte limit."""
    return f"er_{tenant_namespace(org)}"


@dataclass(frozen=True)
class TenantPlan:
    """Everything derived for one org, computed once and passed around."""

    org: str
    #: Namespace == metadata schema == config ``tenant``. Kept per-tenant even
    #: though the database is dedicated: if a stale DSN ever points at another
    #: tenant's database, a per-tenant schema surfaces the fault as an empty
    #: namespace instead of silently attaching their real catalog.
    tenant: str
    db_name: str
    catalog_dsn: str
    data_path: str
    config_path: str
    drop_root: str
    env: dict[str, str]


def _required(settings: ServerSettings, attribute: str, env_var: str) -> str:
    value = getattr(settings, attribute)
    if value is None or not str(value).strip():
        raise ProvisioningNotConfigured(
            f"automatic org provisioning requires {env_var}; set it or pass config_path"
        )
    return str(value)


def plan_for(settings: ServerSettings, org: str) -> TenantPlan:
    """Derive the org's plan, or refuse naming the first missing setting."""
    maint_dsn = _required(settings, "maint_dsn", "ERSERVER_MAINT_DSN")
    dsn_template = _required(settings, "tenant_dsn_template", "ERSERVER_TENANT_DSN_TEMPLATE")
    if "{dbname}" not in dsn_template:
        raise ProvisioningNotConfigured("ERSERVER_TENANT_DSN_TEMPLATE must contain {dbname}")
    data_path_template = settings.lake_data_path_template
    if "{ns}" not in data_path_template or not data_path_template.endswith("/"):
        raise ProvisioningNotConfigured(
            "ERSERVER_LAKE_DATA_PATH_TEMPLATE must contain {ns} and end with /"
        )
    config_root = _required(settings, "config_root", "ERSERVER_CONFIG_ROOT")
    drop_root = _required(settings, "drop_root", "ERSERVER_DROP_ROOT")
    del maint_dsn  # only its presence is checked here; the runner reads the env

    tenant = tenant_namespace(org)
    db_name = tenant_db_name(org)
    catalog_dsn = dsn_template.format(dbname=db_name)
    data_path = data_path_template.format(ns=tenant)
    return TenantPlan(
        org=org,
        tenant=tenant,
        db_name=db_name,
        catalog_dsn=catalog_dsn,
        data_path=data_path,
        config_path=str(Path(config_root) / f"{org}.yaml"),
        drop_root=str(Path(drop_root) / org),
        env={
            **settings.tenant_env_extra,
            "ER_CATALOG_DSN": catalog_dsn,
            "ER_LAKE_DATA_PATH": data_path,
            "ER_LAKE_METADATA_SCHEMA": tenant,
        },
    )


def seed_template_text(settings: ServerSettings) -> str:
    """The seed template's text; the repo's ``configs/default.yaml`` by default."""
    if settings.config_template_path is not None:
        template = Path(settings.config_template_path)
    else:
        template = Path(__file__).resolve().parents[3] / "configs" / "default.yaml"
    if not template.is_file():
        raise ProvisioningNotConfigured(
            f"seed config template not found at {template}; set ERSERVER_CONFIG_TEMPLATE"
        )
    return template.read_text(encoding="utf-8")


def render_seed_config(template_text: str, plan: TenantPlan) -> str:
    """Template the tenant-identity fields into the seed document and validate it.

    Raises :class:`er.config.loader.ConfigValidationError` — a broken template
    fails the ``POST``, never the provision job.
    """
    import yaml

    from erserver import configsvc

    document = yaml.safe_load(template_text)
    if not isinstance(document, dict) or "storage" not in document:
        raise ProvisioningNotConfigured("seed config template is not an S6 document")
    document["tenant"] = plan.tenant
    document["storage"]["data_path"] = plan.data_path
    document["storage"]["drop_dir"] = plan.drop_root
    # Under the tenant's own prefix, so a future purge is one prefix delete.
    document["storage"]["model_uri_prefix"] = f"{plan.data_path}models/"
    rendered = yaml.safe_dump(document, sort_keys=False)
    configsvc.validate_yaml(rendered)
    return rendered


def _write_config_file(config_path: str, yaml_text: str) -> None:
    """Atomic swap, as configsvc.publish does: a runner starting mid-seed
    reads a whole document or none."""
    target = Path(config_path)
    target.parent.mkdir(parents=True, exist_ok=True)
    descriptor, staged = tempfile.mkstemp(dir=target.parent, suffix=".yaml.tmp")
    with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
        handle.write(yaml_text)
    os.replace(staged, target)


def _seed_lock_key(org: str) -> int:
    """A signed-bigint advisory key in its own domain — same recipe as the
    engine's tenant lock (er.lake.catalog.advisory_lock_key), different
    prefix so the two lock spaces cannot collide."""
    digest = hashlib.blake2b(f"erserver:seed:{org}".encode(), digest_size=8).digest()
    return int.from_bytes(digest, "big", signed=True)


def seed_config(
    connection: psycopg.Connection,
    org: str,
    yaml_text: str,
    actor: str,
    *,
    config_path: str,
) -> dict[str, Any]:
    """Publish the org's version 1 without enqueueing what publish would.

    :func:`erserver.configsvc.publish` enqueues train + full-run jobs — wrong
    for a tenant with no data, and refused by the state guard anyway — so this
    replicates its publish bookkeeping (stamp published, set the active
    version, atomic file write, correction-schedule sync) minus the jobs.

    Serialized per org by a session advisory lock, so concurrent onboarding
    POSTs produce exactly one published version. A replay rewrites the config
    file from the *stored* published version — which both repairs a crash
    between the publish commit and the file write, and keeps the file matching
    whatever version is actually live.
    """
    from erserver import configsvc, schedules

    key = _seed_lock_key(org)
    with connection.cursor() as cursor:
        cursor.execute("SELECT pg_advisory_lock(%s)", (key,))
    try:
        with connection.cursor(row_factory=dict_row) as cursor:
            cursor.execute(
                "SELECT version, yaml FROM config_versions "
                "WHERE org = %s AND state = 'published' ORDER BY version DESC LIMIT 1",
                (org,),
            )
            existing = cursor.fetchone()
        if existing is not None:
            _write_config_file(config_path, str(existing["yaml"]))
            return {"org": org, "version": int(existing["version"]), "replayed": True}

        config, _ = configsvc.validate_yaml(yaml_text)
        version = int(configsvc.create_version(connection, org, yaml_text, actor)["version"])
        # File before the publish commit: if either side dies, the replay path
        # above reconverges (an unpublished draft is superseded by re-seeding;
        # a published version rewrites its file).
        _write_config_file(config_path, yaml_text)
        with connection.cursor() as cursor:
            cursor.execute(
                "UPDATE config_versions SET state = 'published', tier = 'C', "
                "published_at = now() WHERE org = %s AND version = %s",
                (org, version),
            )
            cursor.execute(
                "UPDATE orgs SET active_config_version = %s WHERE name = %s", (version, org)
            )
        connection.commit()

        schedules.sync_correction_schedule(connection, org, config.correction_pass.cadence)
        return {"org": org, "version": version, "replayed": False}
    finally:
        # An aborted transaction would refuse the unlock statement; the lock
        # itself is session-scoped, so a dead connection releases it anyway.
        connection.rollback()
        with connection.cursor() as cursor:
            cursor.execute("SELECT pg_advisory_unlock(%s)", (key,))
        connection.commit()


def create_tenant_database(maint_dsn: str, db_name: str) -> bool:
    """Create the org's dedicated database; returns whether it was created.

    ``CREATE DATABASE`` cannot run inside a transaction (hence autocommit) and
    Postgres has no ``IF NOT EXISTS`` for it (hence the probe); a lost race to
    another attempt reads as already-exists, which is the same success.
    """
    with psycopg.connect(maint_dsn, autocommit=True) as connection:
        with connection.cursor() as cursor:
            cursor.execute("SELECT 1 FROM pg_database WHERE datname = %s", (db_name,))
            if cursor.fetchone() is not None:
                return False
        try:
            connection.execute(sql.SQL("CREATE DATABASE {}").format(sql.Identifier(db_name)))
        except pg_errors.DuplicateDatabase:
            return False
    return True


def _fault(exc: BaseException) -> tuple[int, str | None, str]:
    """Map one step's failure onto the S4.0/S4.7 taxonomy the retry matrix reads."""
    if isinstance(exc, ProvisioningNotConfigured | KeyError):
        return 2, "config", f"provision job misconfigured: {exc}"
    if isinstance(exc, pg_errors.InsufficientPrivilege):
        return 2, "config", f"maintenance role lacks CREATEDB: {exc}"
    if isinstance(exc, psycopg.OperationalError):
        return 1, "transient_io", f"catalog cluster unreachable: {exc}"
    if isinstance(exc, duckdb.IOException):
        return 1, "transient_io", str(exc)
    if isinstance(exc, ErError | ConfigValidationError | EnvError):
        # The engine's own convention (er.service._refusal): code from the
        # exception, class from the taxonomy.
        return exit_code_for(exc), classify(exc).value, str(exc)
    return 1, None, f"{type(exc).__name__}: {exc}"


def _emit_stage(outcome: StageOutcome, duration_ms: int) -> None:
    """One S5.2-shaped JSON line on stderr, so ``jobs.progress`` fills for free."""
    record = {
        "stage": outcome.stage,
        "status": outcome.status,
        "exit_code": outcome.exit_code,
        "duration_ms": duration_ms,
        "error_class": outcome.error_class,
        "error_detail": outcome.error_detail,
    }
    sys.stderr.write(json.dumps(record, separators=(",", ":")) + "\n")
    sys.stderr.flush()


def _step_create_database(params: dict[str, Any]) -> None:
    maint_dsn = os.environ.get(MAINT_DSN_VAR)
    if maint_dsn is None or not maint_dsn.strip():
        raise ProvisioningNotConfigured(
            f"{MAINT_DSN_VAR} is not set in the runner environment"
        )
    create_tenant_database(maint_dsn, str(params["db_name"]))


def _step_init_lake(params: dict[str, Any]) -> None:
    # Deferred so a job that fails CREATE DATABASE never pays the engine's
    # import graph; the tenant's ER_* env is already in this process.
    from er.lake.init import init_lake

    init_lake(tenant=str(params["tenant"]))


def execute(params: dict[str, Any], *, run_id: str) -> RunOutcome:
    """Run the provision job: dedicated database, then ``er init``.

    Returns the same frozen :class:`er.service.RunOutcome` every other kind
    returns, so the runner's terminal-record contract and the dispatcher's
    retry matrix apply untouched.
    """
    steps = (("create_database", _step_create_database), ("init_lake", _step_init_lake))
    stages: list[StageOutcome] = []
    for name, step in steps:
        started = time.monotonic()
        try:
            step(params)
        except Exception as exc:  # mapped, never raised: the runner exits by code
            code, error_class, detail = _fault(exc)
            failed = StageOutcome(
                stage=name,
                exit_code=code,
                status="failed",
                error_class=error_class,
                error_detail=detail,
            )
            stages.append(failed)
            _emit_stage(failed, int((time.monotonic() - started) * 1000))
            return RunOutcome(
                run_id=run_id,
                mode="provision",
                exit_code=code,
                error_class=error_class,
                error_detail=detail,
                stages=tuple(stages),
            )
        succeeded = StageOutcome(
            stage=name, exit_code=0, status="succeeded", error_class=None, error_detail=None
        )
        stages.append(succeeded)
        _emit_stage(succeeded, int((time.monotonic() - started) * 1000))
    return RunOutcome(
        run_id=run_id,
        mode="provision",
        exit_code=0,
        error_class=None,
        error_detail=None,
        stages=tuple(stages),
    )
