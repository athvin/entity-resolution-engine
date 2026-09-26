"""The FastAPI control plane: orgs, keys, jobs, config, schedules, reads, steward.

docs/backend-design.md §6 at walking-skeleton-plus depth. The API never
executes pipeline stages (§4): mutations write control-plane rows and return;
the dispatcher does the rest. Reads open read-only DuckLake attaches under the
org's env overlay (§8) and never take the writer lock; steward writes try the
lock inline and stage on conflict (§16 problem 2).

AuthZ is one middleware-shaped dependency (§9): a bearer credential resolves
to a Principal, and every org-scoped route calls ``principal.require(org,
role)`` — cross-tenant access fails closed on the principal's own org, never
on handler discipline.

No ``from __future__ import annotations`` here, deliberately: FastAPI resolves
postponed annotations against module globals, and the dependency aliases are
factory-local — stringified, they degrade into required query parameters.
"""

import csv
import io
from collections.abc import Iterator
from contextlib import asynccontextmanager, contextmanager
from datetime import datetime
from pathlib import Path
from typing import Annotated, Any

import duckdb
import psycopg
from fastapi import Depends, FastAPI, Header, HTTPException, Query, Response, UploadFile
from pydantic import BaseModel, Field
from ulid import ULID

from er.config.loader import ConfigValidationError, load_config
from er.errors import ErError, exit_code_for
from er.lake.env import EnvError
from erserver import configsvc, db, provision, queue, readapi, schedules, steward, webhooks
from erserver.auth import AuthError, Principal, audit, authenticate, issue_key, revoke_key
from erserver.policy import JOB_KINDS, STATES
from erserver.secrets import UnresolvedSecretError, resolve_env
from erserver.settings import ServerSettings

__all__ = ["create_app"]

#: Hard ceiling on one import delivery. Bulk history belongs to a connector's
#: initial sync straight into the drop dir, not to a request body.
MAX_IMPORT_BYTES = 256 * 1024 * 1024
_IMPORT_CHUNK_BYTES = 1024 * 1024


# --------------------------------------------------------------------------- #
# request/response models
# --------------------------------------------------------------------------- #


class OrgIn(BaseModel):
    name: str = Field(min_length=1, max_length=128, pattern=r"^[a-z0-9][a-z0-9_-]*$")
    #: Present: manual/BYO mode — the operator provisioned the lake out of band
    #: and supplies everything. Absent: auto mode — the server derives the
    #: namespace, seeds the config, and enqueues a provision job.
    config_path: str | None = None
    env: dict[str, str] = Field(default_factory=dict)
    drop_root: str | None = None


class KeyIn(BaseModel):
    role: str


class JobIn(BaseModel):
    kind: str
    params: dict[str, Any] = Field(default_factory=dict)
    priority: int = 0
    max_attempts: int = Field(default=3, ge=1, le=10)


class JobOut(BaseModel):
    job_id: str
    org: str
    kind: str
    params: dict[str, Any]
    state: str
    priority: int
    idempotency_key: str | None
    run_id: str | None
    attempt: int
    max_attempts: int
    exit_code: int | None
    error_class: str | None
    error_detail: str | None
    outcome: str | None
    progress: dict[str, Any]

    @classmethod
    def of(cls, job: queue.Job) -> "JobOut":
        return cls(**vars(job))


class ScheduleIn(BaseModel):
    kind: str
    cron: str
    params: dict[str, Any] = Field(default_factory=dict)


class ConfigVersionIn(BaseModel):
    yaml: str


class WebhookIn(BaseModel):
    url: str
    events: list[str] = Field(default_factory=lambda: ["job.completed"])
    secret: str | None = None


class ReviewResolveIn(BaseModel):
    resolution: str  # match | no_match | dismiss


class AssertionIn(BaseModel):
    kind: str  # always | never
    a: str
    b: str
    note: str | None = None
    apply_now: bool = False


# --------------------------------------------------------------------------- #
# app factory
# --------------------------------------------------------------------------- #


def create_app(settings: ServerSettings | None = None) -> FastAPI:
    """Build the app; ``settings`` defaults to the ``ERSERVER_*`` environment."""
    resolved = settings if settings is not None else ServerSettings.from_env()

    @asynccontextmanager
    async def lifespan(_: FastAPI):  # type: ignore[no-untyped-def]
        bootstrap = db.connect(resolved.dsn)
        try:
            db.ensure_schema(bootstrap)
        finally:
            bootstrap.close()
        yield

    app = FastAPI(title="er control plane", version="0.2.0", lifespan=lifespan)

    def connection() -> Iterator[psycopg.Connection]:
        conn = db.connect(resolved.dsn)
        try:
            yield conn
        finally:
            conn.close()

    Conn = Annotated[psycopg.Connection, Depends(connection)]

    def principal_of(
        conn: Conn,
        authorization: Annotated[str | None, Header(alias="Authorization")] = None,
    ) -> Principal:
        try:
            return authenticate(conn, authorization, operator_token=resolved.operator_token)
        except AuthError as exc:
            raise HTTPException(exc.status, str(exc)) from exc

    Caller = Annotated[Principal, Depends(principal_of)]

    def guard(caller: Principal, org: str, minimum: str) -> None:
        try:
            caller.require(org, minimum)
        except AuthError as exc:
            raise HTTPException(exc.status, str(exc)) from exc

    def org_or_404(conn: psycopg.Connection, org: str) -> dict[str, Any]:
        record = queue.org_record(conn, org)
        if record is None:
            raise HTTPException(404, f"no org {org!r}")
        return record

    def org_tenant(record: dict[str, Any]) -> str:
        try:
            return str(load_config(Path(record["config_path"])).tenant)
        except (ConfigValidationError, OSError) as exc:
            raise HTTPException(
                409, f"org config at {record['config_path']} is not loadable: {exc}"
            ) from exc

    # ---------------------------------------------------------------- system

    @app.get("/healthz")
    def healthz() -> dict[str, str]:
        return {"status": "ok"}

    # ------------------------------------------------------- operator: orgs

    @app.post("/v1/orgs", status_code=201)
    def create_org(body: OrgIn, conn: Conn, caller: Caller) -> dict[str, Any]:
        guard(caller, body.name, "operator")
        if body.config_path is not None:
            # Manual/BYO mode: the operator provisioned the lake out of band,
            # so the org starts active. A replay never touches state.
            queue.ensure_org(
                conn,
                body.name,
                config_path=body.config_path,
                env=body.env,
                drop_root=body.drop_root,
                state="active",
            )
            audit(conn, caller.actor, body.name, "org.upsert", {"config_path": body.config_path})
            return {"name": body.name}
        # Auto mode: derive, seed, issue, enqueue. Every step is idempotent, so
        # a replayed POST resumes onboarding instead of stranding the org.
        try:
            plan = provision.plan_for(resolved, body.name)
            template_text = provision.seed_template_text(resolved)
            yaml_text = provision.render_seed_config(template_text, plan)
        except provision.ProvisioningNotConfigured as exc:
            raise HTTPException(503, str(exc)) from exc
        except ConfigValidationError as exc:
            raise HTTPException(
                500, f"seed config template invalid at {exc.pointer}: {exc}"
            ) from exc
        # The insert is the mutex: two racing POSTs get exactly one True, so
        # the one-time admin key below is issued exactly once.
        first_time = queue.register_org(
            conn,
            body.name,
            config_path=plan.config_path,
            env=plan.env,
            drop_root=plan.drop_root,
            state="provisioning",
        )
        Path(plan.drop_root).mkdir(parents=True, exist_ok=True)
        seeded = provision.seed_config(
            conn, body.name, yaml_text, caller.actor, config_path=plan.config_path
        )
        response: dict[str, Any] = {
            "name": body.name,
            "tenant": plan.tenant,
            "config_version": seeded["version"],
        }
        if first_time:
            # Shown once, on the first POST only — a replay never re-issues.
            key_id, admin_key = issue_key(conn, body.name, "admin")
            response["admin_key_id"] = key_id
            response["admin_key"] = admin_key
        job = queue.enqueue(
            conn,
            body.name,
            "provision",
            params={"tenant": plan.tenant, "db_name": plan.db_name, "data_path": plan.data_path},
            idempotency_key=f"provision:{body.name}",
            max_attempts=3,
        )
        audit(
            conn,
            caller.actor,
            body.name,
            "org.provision",
            {"db_name": plan.db_name, "tenant": plan.tenant, "job_id": job.job_id},
        )
        response["job_id"] = job.job_id
        response["state"] = queue.org_state(conn, body.name)
        return response

    @app.get("/v1/orgs/{org}")
    def org_detail(org: str, conn: Conn, caller: Caller) -> dict[str, Any]:
        """The onboarding poll target: is my org active yet?"""
        guard(caller, org, "viewer")
        record = org_or_404(conn, org)
        return {
            "name": record["name"],
            "state": record["state"],
            "active_config_version": record["active_config_version"],
            "drop_root": record["drop_root"],
        }

    @app.post("/v1/orgs/{org}/api-keys", status_code=201)
    def create_key(org: str, body: KeyIn, conn: Conn, caller: Caller) -> dict[str, str]:
        guard(caller, org, "operator")
        org_or_404(conn, org)
        try:
            key_id, full_key = issue_key(conn, org, body.role)
        except AuthError as exc:
            raise HTTPException(exc.status, str(exc)) from exc
        audit(conn, caller.actor, org, "key.issue", {"key_id": key_id, "role": body.role})
        return {"key_id": key_id, "key": full_key, "role": body.role}

    @app.delete("/v1/orgs/{org}/api-keys/{key_id}")
    def delete_key(org: str, key_id: str, conn: Conn, caller: Caller) -> dict[str, bool]:
        guard(caller, org, "operator")
        revoked = revoke_key(conn, org, key_id)
        audit(conn, caller.actor, org, "key.revoke", {"key_id": key_id, "revoked": revoked})
        return {"revoked": revoked}

    # ---------------------------------------------------------------- jobs

    @app.post("/v1/orgs/{org}/jobs", status_code=202)
    def submit_job(
        org: str,
        body: JobIn,
        conn: Conn,
        caller: Caller,
        idempotency_key: Annotated[str | None, Header(alias="Idempotency-Key")] = None,
    ) -> JobOut:
        guard(caller, org, "steward")
        if idempotency_key is None or not idempotency_key.strip():
            raise HTTPException(400, "Idempotency-Key header is required")
        if body.kind not in JOB_KINDS:
            raise HTTPException(422, f"unknown job kind {body.kind!r}; one of {list(JOB_KINDS)}")
        if body.kind == "provision" and caller.role != "operator":
            raise HTTPException(403, "provision jobs are operator-only")
        try:
            job = queue.enqueue(
                conn,
                org,
                body.kind,
                params=body.params,
                idempotency_key=idempotency_key.strip(),
                priority=body.priority,
                max_attempts=body.max_attempts,
            )
        except queue.UnknownOrgError as exc:
            raise HTTPException(404, str(exc)) from exc
        except queue.OrgNotActiveError as exc:
            raise HTTPException(409, str(exc)) from exc
        audit(conn, caller.actor, org, "job.submit", {"job_id": job.job_id, "kind": body.kind})
        return JobOut.of(job)

    @app.get("/v1/orgs/{org}/jobs")
    def jobs_index(
        org: str,
        conn: Conn,
        caller: Caller,
        state: Annotated[str | None, Query()] = None,
        limit: Annotated[int, Query(ge=1, le=200)] = 50,
    ) -> list[JobOut]:
        guard(caller, org, "viewer")
        if state is not None and state not in STATES:
            raise HTTPException(422, f"unknown state {state!r}; one of {list(STATES)}")
        return [JobOut.of(job) for job in queue.list_jobs(conn, org, state=state, limit=limit)]

    @app.get("/v1/orgs/{org}/jobs/{job_id}")
    def job_detail(org: str, job_id: str, conn: Conn, caller: Caller) -> JobOut:
        guard(caller, org, "viewer")
        job = queue.get_job(conn, org, job_id)
        if job is None:
            raise HTTPException(404, f"no job {job_id!r} in org {org!r}")
        return JobOut.of(job)

    @app.post("/v1/orgs/{org}/jobs/{job_id}:cancel")
    def cancel_job(org: str, job_id: str, conn: Conn, caller: Caller) -> dict[str, Any]:
        guard(caller, org, "steward")
        state = queue.cancel(conn, org, job_id)
        if state is None:
            raise HTTPException(409, "job is not cancelable (already terminal or unknown)")
        audit(conn, caller.actor, org, "job.cancel", {"job_id": job_id, "state": state})
        return {"job_id": job_id, "state": state}

    @app.post("/v1/orgs/{org}/jobs/{job_id}:resume")
    def resume_job(org: str, job_id: str, conn: Conn, caller: Caller) -> dict[str, Any]:
        guard(caller, org, "steward")
        if not queue.resume_job(conn, org, job_id):
            raise HTTPException(409, "job is not resumable (not failed or canceled)")
        audit(conn, caller.actor, org, "job.resume", {"job_id": job_id})
        return {"job_id": job_id, "state": "queued"}

    # ------------------------------------------------------------ schedules

    @app.post("/v1/orgs/{org}/schedules", status_code=201)
    def create_schedule(org: str, body: ScheduleIn, conn: Conn, caller: Caller) -> dict[str, Any]:
        guard(caller, org, "admin")
        org_or_404(conn, org)
        try:
            schedule = schedules.create(
                conn, org, kind=body.kind, cron=body.cron, params=body.params
            )
        except ValueError as exc:
            raise HTTPException(422, str(exc)) from exc
        audit(conn, caller.actor, org, "schedule.create", {"schedule_id": schedule.schedule_id})
        return {"schedule_id": schedule.schedule_id, "kind": schedule.kind, "cron": schedule.cron}

    @app.get("/v1/orgs/{org}/schedules")
    def schedules_index(org: str, conn: Conn, caller: Caller) -> list[dict[str, Any]]:
        guard(caller, org, "viewer")
        return [
            {
                "schedule_id": s.schedule_id,
                "kind": s.kind,
                "cron": s.cron,
                "enabled": s.enabled,
                "source": s.source,
                "last_enqueued_at": s.last_enqueued_at,
            }
            for s in schedules.list_schedules(conn, org)
        ]

    @app.delete("/v1/orgs/{org}/schedules/{schedule_id}")
    def delete_schedule(org: str, schedule_id: str, conn: Conn, caller: Caller) -> dict[str, bool]:
        guard(caller, org, "admin")
        deleted = schedules.delete(conn, org, schedule_id)
        audit(conn, caller.actor, org, "schedule.delete", {"schedule_id": schedule_id})
        return {"deleted": deleted}

    # --------------------------------------------------------------- config

    @app.post("/v1/orgs/{org}/config/versions", status_code=201)
    def create_config_version(
        org: str, body: ConfigVersionIn, conn: Conn, caller: Caller
    ) -> dict[str, Any]:
        guard(caller, org, "admin")
        org_or_404(conn, org)
        try:
            version = configsvc.create_version(conn, org, body.yaml, caller.actor)
        except ConfigValidationError as exc:
            raise HTTPException(422, f"config error at {exc.pointer}: {exc}") from exc
        audit(conn, caller.actor, org, "config.draft", {"version": version["version"]})
        return version

    @app.get("/v1/orgs/{org}/config/versions")
    def config_versions_index(org: str, conn: Conn, caller: Caller) -> list[dict[str, Any]]:
        guard(caller, org, "viewer")
        return configsvc.list_versions(conn, org)

    @app.get("/v1/orgs/{org}/config")
    def active_config(org: str, conn: Conn, caller: Caller) -> dict[str, Any]:
        guard(caller, org, "viewer")
        active = configsvc.active_config(conn, org)
        if active is None:
            raise HTTPException(404, "no published config")
        return active

    @app.post("/v1/orgs/{org}/config/versions/{version}:publish")
    def publish_config(org: str, version: int, conn: Conn, caller: Caller) -> dict[str, Any]:
        guard(caller, org, "admin")
        org_or_404(conn, org)
        try:
            result = configsvc.publish(
                conn,
                org,
                version,
                actor=caller.actor,
                is_operator=caller.role == "operator",
            )
        except configsvc.PublishRefused as exc:
            raise HTTPException(exc.status, str(exc)) from exc
        except ConfigValidationError as exc:
            raise HTTPException(422, f"config error at {exc.pointer}: {exc}") from exc
        except queue.OrgNotActiveError as exc:
            raise HTTPException(409, str(exc)) from exc
        audit(conn, caller.actor, org, "config.publish", result)
        return result

    # -------------------------------------------------------------- webhooks

    @app.post("/v1/orgs/{org}/webhooks", status_code=201)
    def create_webhook(org: str, body: WebhookIn, conn: Conn, caller: Caller) -> dict[str, Any]:
        guard(caller, org, "admin")
        org_or_404(conn, org)
        hook = webhooks.create(conn, org, url=body.url, events=body.events, secret=body.secret)
        audit(conn, caller.actor, org, "webhook.create", {"webhook_id": hook.webhook_id})
        return {"webhook_id": hook.webhook_id, "url": hook.url, "events": hook.events}

    @app.get("/v1/orgs/{org}/webhooks")
    def webhooks_index(org: str, conn: Conn, caller: Caller) -> list[dict[str, Any]]:
        guard(caller, org, "viewer")
        return [
            {"webhook_id": h.webhook_id, "url": h.url, "events": h.events, "enabled": h.enabled}
            for h in webhooks.list_webhooks(conn, org)
        ]

    @app.delete("/v1/orgs/{org}/webhooks/{webhook_id}")
    def delete_webhook(org: str, webhook_id: str, conn: Conn, caller: Caller) -> dict[str, bool]:
        guard(caller, org, "admin")
        return {"deleted": webhooks.delete(conn, org, webhook_id)}

    # --------------------------------------------------------------- imports

    @app.post("/v1/orgs/{org}/imports", status_code=202)
    def create_import(
        org: str,
        source: Annotated[str, Query(min_length=1)],
        file: UploadFile,
        conn: Conn,
        caller: Caller,
    ) -> dict[str, Any]:
        """The drop-dir connector's push seam: file in, incremental run enqueued."""
        guard(caller, org, "steward")
        record = org_or_404(conn, org)
        if not record["drop_root"]:
            raise HTTPException(409, f"org {org!r} has no drop_root configured")
        delivery_id = str(ULID())
        suffix = Path(file.filename or "delivery.csv").suffix or ".csv"
        target_dir = Path(record["drop_root"]) / source
        target_dir.mkdir(parents=True, exist_ok=True)
        target = target_dir / f"{delivery_id}{suffix}"
        written = 0
        with target.open("wb") as sink:
            while chunk := file.file.read(_IMPORT_CHUNK_BYTES):
                written += len(chunk)
                if written > MAX_IMPORT_BYTES:
                    sink.close()
                    target.unlink(missing_ok=True)
                    raise HTTPException(
                        413,
                        f"delivery exceeds {MAX_IMPORT_BYTES} bytes; "
                        "sync bulk history into the drop dir directly",
                    )
                sink.write(chunk)
        try:
            job = queue.enqueue(
                conn,
                org,
                "run_all_incremental",
                params={"source": source, "path": record["drop_root"]},
                idempotency_key=f"import:{delivery_id}",
            )
        except queue.OrgNotActiveError as exc:
            target.unlink(missing_ok=True)
            raise HTTPException(409, str(exc)) from exc
        audit(
            conn,
            caller.actor,
            org,
            "import.create",
            {"delivery_id": delivery_id, "source": source, "job_id": job.job_id},
        )
        return {"delivery_id": delivery_id, "file": str(target), "job": JobOut.of(job)}

    # -------------------------------------------------------------- read path

    def org_env(conn: psycopg.Connection, org: str) -> dict[str, str]:
        record = org_or_404(conn, org)
        try:
            return resolve_env(dict(record["env"] or {}))
        except UnresolvedSecretError as exc:
            raise HTTPException(503, str(exc)) from exc

    @contextmanager
    def lake_read(env: dict[str, str]) -> Iterator[duckdb.DuckDBPyConnection]:
        """A tenant read connection whose faults answer as HTTP, not tracebacks.

        A missing/invalid lake environment or an unreachable substrate is the
        operator's problem, not the caller's: 503. Query-shape faults inside
        the body map the same way; deliberate HTTP answers pass through.
        """
        try:
            with readapi.open_lake(env) as lake:
                yield lake
        except HTTPException:
            raise
        except EnvError as exc:
            raise HTTPException(503, f"tenant lake unavailable: {exc}") from exc
        except duckdb.Error as exc:
            raise HTTPException(503, f"tenant lake query failed: {type(exc).__name__}") from exc

    @app.get("/v1/orgs/{org}/golden-records")
    def golden_records(
        org: str,
        conn: Conn,
        caller: Caller,
        q: Annotated[str | None, Query()] = None,
        cursor: Annotated[str | None, Query()] = None,
        limit: Annotated[int, Query(ge=1, le=200)] = 50,
    ) -> dict[str, Any]:
        guard(caller, org, "viewer")
        env = org_env(conn, org)
        snapshot: int | None = None
        after: str | None = None
        if cursor is not None:
            try:
                snapshot, after = readapi.decode_cursor(cursor)
            except ValueError as exc:
                raise HTTPException(422, str(exc)) from exc
        with lake_read(env) as lake:
            rows, snap, next_cursor = readapi.golden_list(
                lake, snapshot=snapshot, q=q, after_key=after, limit=limit
            )
        return {"items": rows, "snapshot": snap, "next_cursor": next_cursor}

    @app.get("/v1/orgs/{org}/golden-records/{entity_id}")
    def golden_record(org: str, entity_id: str, conn: Conn, caller: Caller) -> dict[str, Any]:
        guard(caller, org, "viewer")
        env = org_env(conn, org)
        with lake_read(env) as lake:
            detail = readapi.entity_detail(lake, entity_id)
        if detail is None:
            raise HTTPException(404, f"no entity {entity_id!r}")
        return detail

    @app.get("/v1/orgs/{org}/duplicates")
    def duplicates(
        org: str,
        conn: Conn,
        caller: Caller,
        cursor: Annotated[str | None, Query()] = None,
        limit: Annotated[int, Query(ge=1, le=200)] = 50,
    ) -> dict[str, Any]:
        guard(caller, org, "viewer")
        env = org_env(conn, org)
        snapshot: int | None = None
        after: str | None = None
        if cursor is not None:
            try:
                snapshot, after = readapi.decode_cursor(cursor)
            except ValueError as exc:
                raise HTTPException(422, str(exc)) from exc
        with lake_read(env) as lake:
            rows, snap, next_cursor = readapi.duplicate_groups(
                lake, snapshot=snapshot, after_key=after, limit=limit
            )
        return {"items": rows, "snapshot": snap, "next_cursor": next_cursor}

    @app.get("/v1/orgs/{org}/runs")
    def runs(org: str, conn: Conn, caller: Caller) -> list[dict[str, Any]]:
        guard(caller, org, "viewer")
        env = org_env(conn, org)
        with lake_read(env) as lake:
            return readapi.runs_list(lake)

    @app.get("/v1/orgs/{org}/metrics")
    def org_metrics(org: str, conn: Conn, caller: Caller) -> dict[str, Any]:
        guard(caller, org, "viewer")
        env = org_env(conn, org)
        with lake_read(env) as lake:
            return readapi.metrics(lake)

    @app.get("/v1/orgs/{org}/merge-plans")
    def merge_plan_export(
        org: str,
        conn: Conn,
        caller: Caller,
        since: Annotated[str | None, Query()] = None,
        format: Annotated[str, Query(pattern="^(json|csv)$")] = "json",
    ) -> Response:
        guard(caller, org, "viewer")
        if since is not None:
            try:
                datetime.fromisoformat(since)
            except ValueError as exc:
                raise HTTPException(
                    422, "since must be an ISO 8601 timestamp, e.g. 2026-09-25T00:00:00"
                ) from exc
        env = org_env(conn, org)
        with lake_read(env) as lake:
            plans = readapi.merge_plans(lake, since=since)
        if format == "json":
            import json as json_module

            return Response(
                json_module.dumps({"items": plans}, default=str),
                media_type="application/json",
            )
        buffer = io.StringIO()
        writer = csv.writer(buffer)
        writer.writerow(
            [
                "entity_id",
                "master_key",
                "member_count",
                "member_records",
                "golden_given_name",
                "golden_family_name",
                "golden_email",
                "golden_phone_e164",
            ]
        )
        for plan in plans:
            writer.writerow(
                [
                    plan["entity_id"],
                    plan["master_key"],
                    plan["member_count"],
                    ";".join(plan["member_records"]),
                    plan["golden_given_name"],
                    plan["golden_family_name"],
                    plan["golden_email"],
                    plan["golden_phone_e164"],
                ]
            )
        return Response(
            buffer.getvalue(),
            media_type="text/csv",
            headers={"Content-Disposition": "attachment; filename=merge-plans.csv"},
        )

    # --------------------------------------------------------------- steward

    @app.get("/v1/orgs/{org}/reviews")
    def reviews(
        org: str,
        conn: Conn,
        caller: Caller,
        limit: Annotated[int, Query(ge=1, le=500)] = 100,
    ) -> list[dict[str, Any]]:
        guard(caller, org, "viewer")
        env = org_env(conn, org)
        with lake_read(env) as lake:
            from er.review.queue import open_reviews

            rows = open_reviews(lake, limit=limit)
        return [vars(row) for row in rows]

    def _steward_action(
        conn: psycopg.Connection, org: str, caller: Principal, action: dict[str, Any]
    ) -> dict[str, Any]:
        record = org_or_404(conn, org)
        if record["state"] != "active":
            # Before the flip the tenant database may not even exist; refuse
            # here rather than surface a raw connection error from the attach.
            raise HTTPException(
                409,
                f"org {org!r} is {record['state']}; steward actions are accepted only when active",
            )
        env = dict(record["env"] or {})
        tenant = org_tenant(record)
        try:
            status, result = steward.try_apply_or_stage(
                conn, org, env, tenant, action, caller.actor
            )
        except EnvError as exc:
            raise HTTPException(422, str(exc)) from exc
        except (ErError, KeyError, ValueError) as exc:
            raise HTTPException(409 if exit_code_for(exc) == 3 else 422, str(exc)) from exc
        audit(conn, caller.actor, org, f"steward.{action['type']}", {**result, "status": status})
        return {"status": status, **result, "pending_until_next_reconcile": status == "applied"}

    @app.post("/v1/orgs/{org}/reviews/{review_id}:resolve")
    def resolve_review_endpoint(
        org: str, review_id: str, body: ReviewResolveIn, conn: Conn, caller: Caller
    ) -> dict[str, Any]:
        guard(caller, org, "steward")
        if body.resolution not in ("match", "no_match", "dismiss"):
            raise HTTPException(422, "resolution must be match | no_match | dismiss")
        return _steward_action(
            conn,
            org,
            caller,
            {"type": "resolve_review", "review_id": review_id, "resolution": body.resolution},
        )

    @app.post("/v1/orgs/{org}/assertions", status_code=201)
    def add_assertion_endpoint(
        org: str, body: AssertionIn, conn: Conn, caller: Caller
    ) -> dict[str, Any]:
        guard(caller, org, "steward")
        if body.kind not in ("always", "never"):
            raise HTTPException(422, "kind must be always | never")
        result = _steward_action(
            conn,
            org,
            caller,
            {
                "type": "add_assertion",
                "kind": body.kind,
                "a": body.a,
                "b": body.b,
                "note": body.note,
            },
        )
        if body.apply_now:
            try:
                job = queue.enqueue(
                    conn,
                    org,
                    "run_all_incremental",
                    params={"skip_ingest": True},
                    idempotency_key=f"assert-apply:{ULID()}",
                )
            except queue.OrgNotActiveError as exc:
                raise HTTPException(409, str(exc)) from exc
            result["apply_job"] = job.job_id
        return result

    @app.delete("/v1/orgs/{org}/assertions/{assertion_id}")
    def retract_assertion_endpoint(
        org: str, assertion_id: str, conn: Conn, caller: Caller
    ) -> dict[str, Any]:
        guard(caller, org, "steward")
        return _steward_action(
            conn,
            org,
            caller,
            {"type": "retract_assertion", "assertion_id": assertion_id},
        )

    return app
