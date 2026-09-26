"""Control-plane configuration, read from ``ERSERVER_*`` environment variables.

Deliberately separate from the engine's ``ER_*`` set: the control-plane database
is not the DuckLake catalog, and a runner's per-tenant lake environment is
injected per process by the dispatcher (docs/backend-design.md §7), never read
from the server's own environment.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field

__all__ = ["MissingSettingError", "ServerSettings"]


class MissingSettingError(RuntimeError):
    """A required ``ERSERVER_*`` variable is absent or empty."""

    def __init__(self, name: str) -> None:
        super().__init__(f"ERSERVER_SETTING_MISSING: {name}")
        self.name = name


def _require(name: str) -> str:
    value = os.environ.get(name)
    if value is None or not value.strip():
        raise MissingSettingError(name)
    return value


@dataclass(frozen=True)
class ServerSettings:
    """Everything the API and dispatcher need to start."""

    #: Postgres DSN of the control-plane database (orgs, jobs, schedules).
    dsn: str
    #: Dispatcher poll interval when the queue was empty, in seconds.
    poll_seconds: float = 2.0
    #: The platform operator's bearer token; ``None`` disables operator auth
    #: (every operator endpoint then refuses).
    operator_token: str | None = None
    #: Concurrent runner slots. Per-org serialization is constraint-backed in
    #: the queue, so concurrency here only ever parallelizes ACROSS orgs.
    concurrency: int = 2
    #: Maintenance DSN of a role with CREATEDB, used by the provision job to
    #: create each tenant's dedicated catalog database. The runner reads it
    #: from its inherited process environment — a DSN never rides in job rows.
    maint_dsn: str | None = None
    #: DSN template producing each org's ``ER_CATALOG_DSN``; must contain
    #: ``{dbname}``, e.g. ``postgresql://er:pw@catalog:5432/{dbname}``.
    tenant_dsn_template: str | None = None
    #: Per-tenant S3 prefix template; must contain ``{ns}`` and end with ``/``.
    lake_data_path_template: str = "s3://er-lake/{ns}/"
    #: Directory for server-managed org config files: ``{config_root}/{org}.yaml``.
    config_root: str | None = None
    #: Directory for per-org drop dirs: ``{drop_root}/{org}``.
    drop_root: str | None = None
    #: Seed config template; defaults to the repo's ``configs/default.yaml``.
    config_template_path: str | None = None
    #: Extra ``ER_*`` entries merged into every auto-provisioned org's env —
    #: the place for shared S3 credentials as ``secret://`` references.
    tenant_env_extra: dict[str, str] = field(default_factory=dict)

    @classmethod
    def from_env(cls) -> ServerSettings:
        poll = os.environ.get("ERSERVER_POLL_SECONDS")
        workers = os.environ.get("ERSERVER_CONCURRENCY")
        extra_json = os.environ.get("ERSERVER_TENANT_ENV_JSON")
        tenant_env_extra: dict[str, str] = {}
        if extra_json and extra_json.strip():
            parsed = json.loads(extra_json)
            if not isinstance(parsed, dict) or not all(
                isinstance(key, str) and isinstance(value, str) for key, value in parsed.items()
            ):
                raise ValueError("ERSERVER_TENANT_ENV_JSON must be a JSON object of strings")
            tenant_env_extra = parsed
        return cls(
            dsn=_require("ERSERVER_DSN"),
            poll_seconds=float(poll) if poll else 2.0,
            operator_token=os.environ.get("ERSERVER_OPERATOR_TOKEN") or None,
            concurrency=max(1, int(workers)) if workers else 2,
            maint_dsn=os.environ.get("ERSERVER_MAINT_DSN") or None,
            tenant_dsn_template=os.environ.get("ERSERVER_TENANT_DSN_TEMPLATE") or None,
            lake_data_path_template=os.environ.get("ERSERVER_LAKE_DATA_PATH_TEMPLATE")
            or "s3://er-lake/{ns}/",
            config_root=os.environ.get("ERSERVER_CONFIG_ROOT") or None,
            drop_root=os.environ.get("ERSERVER_DROP_ROOT") or None,
            config_template_path=os.environ.get("ERSERVER_CONFIG_TEMPLATE") or None,
            tenant_env_extra=tenant_env_extra,
        )
