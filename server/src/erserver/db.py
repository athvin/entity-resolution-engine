"""The control-plane schema and its connection helper.

Two tables for the walking skeleton (docs/backend-design.md §5, §7):

- ``orgs`` — one row per tenant: the S6 config document the runner loads, and
  the ``ER_*`` environment overrides the dispatcher injects into runner
  processes (metadata schema, data path, credentials references).
- ``jobs`` — the queue and ledger of pipeline work. It complements, never
  duplicates, the in-lake ``runs``/``run_stages`` ledger: a job exists before a
  run starts, and carries only cross-tenant coordination state.

Two constraints do the correctness work, both enforced by Postgres rather than
by code: at most one active job per org (the engine is single-writer per
tenant), and at most one job per ``(org, idempotency_key)``.
"""

from __future__ import annotations

import psycopg

__all__ = ["SCHEMA_STATEMENTS", "connect", "ensure_schema"]

SCHEMA_STATEMENTS: tuple[str, ...] = (
    """
    CREATE TABLE IF NOT EXISTS orgs (
      name        text PRIMARY KEY,
      config_path text NOT NULL,
      env         jsonb NOT NULL DEFAULT '{}'::jsonb,
      created_at  timestamptz NOT NULL DEFAULT now()
    )
    """,
    """
    CREATE TABLE IF NOT EXISTS jobs (
      job_id          text PRIMARY KEY,
      org             text NOT NULL REFERENCES orgs(name),
      kind            text NOT NULL,
      params          jsonb NOT NULL DEFAULT '{}'::jsonb,
      state           text NOT NULL DEFAULT 'queued',
      priority        integer NOT NULL DEFAULT 0,
      idempotency_key text,
      run_id          text,
      attempt         integer NOT NULL DEFAULT 0,
      max_attempts    integer NOT NULL DEFAULT 3,
      not_before      timestamptz,
      exit_code       integer,
      error_class     text,
      error_detail    text,
      outcome         text,
      progress        jsonb NOT NULL DEFAULT '{}'::jsonb,
      created_at      timestamptz NOT NULL DEFAULT now(),
      updated_at      timestamptz NOT NULL DEFAULT now(),
      started_at      timestamptz,
      finished_at     timestamptz
    )
    """,
    # Idempotent job submission: a replayed key returns the original job.
    """
    CREATE UNIQUE INDEX IF NOT EXISTS jobs_idempotency
      ON jobs (org, idempotency_key) WHERE idempotency_key IS NOT NULL
    """,
    # The engine holds one writer lock per tenant; the queue makes double
    # dispatch impossible by construction rather than by dispatcher discipline.
    # Dropped and recreated so the predicate can evolve (a canceling job still
    # holds its org's slot); both statements are idempotent.
    "DROP INDEX IF EXISTS jobs_one_active_per_org",
    """
    CREATE UNIQUE INDEX jobs_one_active_per_org
      ON jobs (org) WHERE state IN ('dispatching', 'running', 'canceling')
    """,
    """
    CREATE INDEX IF NOT EXISTS jobs_claimable
      ON jobs (state, not_before, priority, job_id)
    """,
    # Job attribution (design §7.12): the person or schedule behind every run.
    "ALTER TABLE jobs ADD COLUMN IF NOT EXISTS created_by text",
    "ALTER TABLE jobs ADD COLUMN IF NOT EXISTS schedule_id text",
    # Per-run sizing rides the job row, never orgs.env (infrastructure.md §6.5,
    # §18.10): the §6.5 class the dispatcher selected at claim time, which the
    # kubernetes launcher translates into pod resources and ER_DUCKDB_* env.
    "ALTER TABLE jobs ADD COLUMN IF NOT EXISTS resource_class text",
    # W3C traceparent captured at enqueue (infrastructure.md §10.2): trace
    # context does not survive the queue-table hop by itself, so it is stored
    # here and re-enters the runner's environment at launch.
    "ALTER TABLE jobs ADD COLUMN IF NOT EXISTS trace_context text",
    # Orgs grew columns after the skeleton; additive and idempotent.
    "ALTER TABLE orgs ADD COLUMN IF NOT EXISTS drop_root text",
    "ALTER TABLE orgs ADD COLUMN IF NOT EXISTS active_config_version integer",
    # Org lifecycle (docs/backend-design.md §7). Rows that predate the column
    # were provisioned manually and are therefore active. CHECK has no
    # IF NOT EXISTS, so drop-then-add is the idempotency idiom, as with
    # jobs_one_active_per_org above.
    "ALTER TABLE orgs ADD COLUMN IF NOT EXISTS state text NOT NULL DEFAULT 'active'",
    "ALTER TABLE orgs DROP CONSTRAINT IF EXISTS orgs_state_check",
    """
    ALTER TABLE orgs ADD CONSTRAINT orgs_state_check
      CHECK (state IN ('provisioning', 'active', 'suspended', 'purging'))
    """,
    """
    CREATE TABLE IF NOT EXISTS api_keys (
      key_id     text PRIMARY KEY,
      org        text NOT NULL REFERENCES orgs(name),
      role       text NOT NULL CHECK (role IN ('admin', 'steward', 'viewer')),
      key_hash   text NOT NULL,
      created_at timestamptz NOT NULL DEFAULT now(),
      revoked_at timestamptz
    )
    """,
    """
    CREATE TABLE IF NOT EXISTS audit_log (
      id     bigserial PRIMARY KEY,
      at     timestamptz NOT NULL DEFAULT now(),
      actor  text NOT NULL,
      org    text,
      action text NOT NULL,
      detail jsonb NOT NULL DEFAULT '{}'::jsonb
    )
    """,
    """
    CREATE TABLE IF NOT EXISTS org_events (
      id         bigserial PRIMARY KEY,
      -- CASCADE, unlike the ledgers above: events are announcements, and an
      -- org's removal has nothing left to announce to.
      org        text NOT NULL REFERENCES orgs(name) ON DELETE CASCADE,
      event_type text NOT NULL,
      payload    jsonb NOT NULL DEFAULT '{}'::jsonb,
      created_at timestamptz NOT NULL DEFAULT now()
    )
    """,
    """
    CREATE INDEX IF NOT EXISTS org_events_feed ON org_events (org, id DESC)
    """,
    """
    CREATE TABLE IF NOT EXISTS email_outbox (
      id          bigserial PRIMARY KEY,
      org         text,
      to_addr     text NOT NULL,
      template    text NOT NULL,
      params      jsonb NOT NULL DEFAULT '{}'::jsonb,
      state       text NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'sent', 'failed')),
      attempts    integer NOT NULL DEFAULT 0,
      not_before  timestamptz,
      last_error  text,
      created_at  timestamptz NOT NULL DEFAULT now(),
      sent_at     timestamptz
    )
    """,
    """
    CREATE INDEX IF NOT EXISTS email_outbox_due ON email_outbox (state, not_before)
    """,
    """
    CREATE TABLE IF NOT EXISTS schedules (
      schedule_id      text PRIMARY KEY,
      org              text NOT NULL REFERENCES orgs(name),
      kind             text NOT NULL,
      params           jsonb NOT NULL DEFAULT '{}'::jsonb,
      cron             text NOT NULL,
      enabled          boolean NOT NULL DEFAULT true,
      source           text NOT NULL DEFAULT 'api',
      last_enqueued_at timestamptz,
      created_at       timestamptz NOT NULL DEFAULT now()
    )
    """,
    # One system-managed correction schedule per org, replaced on config publish.
    """
    CREATE UNIQUE INDEX IF NOT EXISTS schedules_one_correction_per_org
      ON schedules (org) WHERE source = 'config:correction_pass'
    """,
    # One system-managed weekly maintenance schedule per org, seeded at provision.
    """
    CREATE UNIQUE INDEX IF NOT EXISTS schedules_one_maintenance_per_org
      ON schedules (org) WHERE source = 'system:lake_maintain'
    """,
    """
    CREATE TABLE IF NOT EXISTS config_versions (
      org          text NOT NULL REFERENCES orgs(name),
      version      integer NOT NULL,
      yaml         text NOT NULL,
      config_hash  text NOT NULL,
      state        text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft', 'published')),
      tier         text,
      created_by   text NOT NULL,
      created_at   timestamptz NOT NULL DEFAULT now(),
      published_at timestamptz,
      PRIMARY KEY (org, version)
    )
    """,
    """
    CREATE TABLE IF NOT EXISTS webhooks (
      webhook_id text PRIMARY KEY,
      org        text NOT NULL REFERENCES orgs(name),
      url        text NOT NULL,
      events     jsonb NOT NULL DEFAULT '["job.completed"]'::jsonb,
      secret     text,
      enabled    boolean NOT NULL DEFAULT true,
      created_at timestamptz NOT NULL DEFAULT now()
    )
    """,
    # Steward actions that arrived while the tenant's writer lock was held by a
    # pipeline run; the dispatcher drains them when the org goes quiet
    # (docs/backend-design.md §16 problem 2).
    """
    CREATE TABLE IF NOT EXISTS staged_steward_actions (
      action_id  text PRIMARY KEY,
      org        text NOT NULL REFERENCES orgs(name),
      action     jsonb NOT NULL,
      state      text NOT NULL DEFAULT 'pending'
                 CHECK (state IN ('pending', 'applied', 'failed')),
      error      text,
      created_by text NOT NULL,
      created_at timestamptz NOT NULL DEFAULT now(),
      applied_at timestamptz
    )
    """,
    """
    CREATE INDEX IF NOT EXISTS staged_pending ON staged_steward_actions (org, state)
    """,
)


def connect(dsn: str) -> psycopg.Connection:
    """One control-plane connection; the caller owns commit and close."""
    return psycopg.connect(dsn)


def ensure_schema(connection: psycopg.Connection) -> None:
    """Apply the skeleton DDL; every statement is idempotent.

    Serialized under an advisory lock: the API and the dispatcher both ensure
    the schema at boot, and two sessions racing the drop-then-recreate index
    idiom (or queueing DDL behind each other's locks) deadlock an otherwise
    healthy first start. The xact-scoped lock releases at commit.
    """
    with connection.cursor() as cursor:
        cursor.execute("SELECT pg_advisory_xact_lock(hashtext('erserver.ensure_schema'), 0)")
        for statement in SCHEMA_STATEMENTS:
            cursor.execute(statement)
    connection.commit()
