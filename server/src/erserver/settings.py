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
    #: Deployment environment. Anything other than ``dev`` disables the FastAPI
    #: docs routes (``/docs``, ``/redoc``, ``/openapi.json``) — the only
    #: unauthenticated surface besides ``/healthz`` (docs/infrastructure.md §4.1).
    environment: str = "dev"
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
    #: Prefix of each tenant's dedicated database name, so server stacks that
    #: share one Postgres cluster derive distinct databases for the same org
    #: (docs/infrastructure.md §7.2). The default keeps today's names.
    tenant_db_prefix: str = "er_"
    #: Per-tenant S3 prefix template; must contain ``{ns}`` and end with ``/``.
    lake_data_path_template: str = "s3://lake/{ns}/"
    #: Directory for server-managed org config files: ``{config_root}/{org}.yaml``.
    config_root: str | None = None
    #: Directory for per-org drop dirs: ``{drop_root}/{org}``.
    drop_root: str | None = None
    #: Seed config template; defaults to the repo's ``configs/default.yaml``.
    config_template_path: str | None = None
    #: Extra ``ER_*`` entries merged into every auto-provisioned org's env —
    #: the place for shared S3 credentials as ``secret://`` references.
    tenant_env_extra: dict[str, str] = field(default_factory=dict)
    #: SMTP relay URL: ``smtp://user:pass@host:port`` (STARTTLS) or
    #: ``smtps://…`` (implicit TLS). The userinfo halves may be ``secret://``
    #: references, resolved at send time. ``None`` disables sending — outbox
    #: rows park as ``queued`` and drain when the relay is configured.
    smtp_url: str | None = None
    #: The From header of every platform email.
    email_from: str | None = None
    #: Link prefix into the frontend, e.g. ``https://app.example.com`` — emails
    #: carry links, never data.
    email_base_url: str | None = None
    #: How runs execute: ``subprocess`` (the single-VM default — the dispatcher
    #: forks ``python -m erserver.runner``) or ``kubernetes`` (one batch/v1 Job
    #: per run behind the same ``launch`` seam; docs/infrastructure.md §6).
    launcher: str = "subprocess"
    #: Image every runner Job runs (``registry/repo:tag`` or ``@digest``).
    #: Required when ``launcher=kubernetes``; never defaulted — an unpinned
    #: runner image is exactly the stale-code hazard §14.2 exists to prevent.
    runner_image: str | None = None
    #: Namespace the runner Jobs are created in. Empty means "the namespace this
    #: dispatcher runs in", read from the ServiceAccount mount at launch time.
    k8s_namespace: str | None = None
    #: Name of the projected Secret (§9: the ExternalSecret target) the runner
    #: pod consumes via ``envFrom`` — the same baseline the dispatcher itself
    #: reads, so ``ERSERVER_SECRET_*`` values never ride the Job manifest.
    #: Empty disables the reference.
    runner_env_secret: str | None = "er-erserver-env"
    #: ServiceAccount the runner pods run as. §6.4 anticipates a per-tenant
    #: value in Phase 4; until then every run shares this one.
    runner_service_account: str | None = None
    #: PVC name and mount path of the shared EFS filesystem (§6.2/§7.4): the
    #: runner opens the org config and lists CSV drops by path, so the mount
    #: path must equal the API's and the dispatcher's.
    runner_efs_claim: str = "er-efs"
    runner_efs_mount_path: str = "/srv/er"
    #: How often the kubernetes launcher polls a Job for completion/cancel.
    k8s_poll_seconds: float = 5.0

    @classmethod
    def from_env(cls) -> ServerSettings:
        poll = os.environ.get("ERSERVER_POLL_SECONDS")
        workers = os.environ.get("ERSERVER_CONCURRENCY")
        launcher = os.environ.get("ERSERVER_LAUNCHER") or "subprocess"
        if launcher not in ("subprocess", "kubernetes"):
            raise ValueError(
                f"ERSERVER_LAUNCHER must be 'subprocess' or 'kubernetes', got {launcher!r}"
            )
        k8s_poll = os.environ.get("ERSERVER_K8S_POLL_SECONDS")
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
            environment=os.environ.get("ERSERVER_ENV") or "dev",
            poll_seconds=float(poll) if poll else 2.0,
            operator_token=os.environ.get("ERSERVER_OPERATOR_TOKEN") or None,
            concurrency=max(1, int(workers)) if workers else 2,
            maint_dsn=os.environ.get("ERSERVER_MAINT_DSN") or None,
            tenant_dsn_template=os.environ.get("ERSERVER_TENANT_DSN_TEMPLATE") or None,
            tenant_db_prefix=os.environ.get("ERSERVER_TENANT_DB_PREFIX") or "er_",
            lake_data_path_template=os.environ.get("ERSERVER_LAKE_DATA_PATH_TEMPLATE")
            or "s3://lake/{ns}/",
            config_root=os.environ.get("ERSERVER_CONFIG_ROOT") or None,
            drop_root=os.environ.get("ERSERVER_DROP_ROOT") or None,
            config_template_path=os.environ.get("ERSERVER_CONFIG_TEMPLATE") or None,
            tenant_env_extra=tenant_env_extra,
            smtp_url=os.environ.get("ERSERVER_SMTP_URL") or None,
            email_from=os.environ.get("ERSERVER_EMAIL_FROM") or None,
            email_base_url=os.environ.get("ERSERVER_EMAIL_BASE_URL") or None,
            launcher=launcher,
            runner_image=os.environ.get("ERSERVER_RUNNER_IMAGE") or None,
            k8s_namespace=os.environ.get("ERSERVER_K8S_NAMESPACE") or None,
            runner_env_secret=os.environ.get("ERSERVER_RUNNER_ENV_SECRET", "er-erserver-env")
            or None,
            runner_service_account=os.environ.get("ERSERVER_RUNNER_SERVICE_ACCOUNT") or None,
            runner_efs_claim=os.environ.get("ERSERVER_RUNNER_EFS_CLAIM") or "er-efs",
            runner_efs_mount_path=os.environ.get("ERSERVER_RUNNER_EFS_MOUNT_PATH") or "/srv/er",
            k8s_poll_seconds=float(k8s_poll) if k8s_poll else 5.0,
        )
