CREATE TABLE IF NOT EXISTS public.jobs (
  job_id text NOT NULL,
  org text NOT NULL REFERENCES orgs(name),
  kind text NOT NULL,
  params jsonb NOT NULL DEFAULT '{}'::jsonb,
  state text NOT NULL DEFAULT 'queued',
  priority bigint NOT NULL DEFAULT 0,
  idempotency_key text,
  run_id text,
  attempt bigint NOT NULL DEFAULT 0,
  max_attempts bigint NOT NULL DEFAULT 3,
  not_before timestamptz,
  created_by text,
  schedule_id text,
  resource_class text,
  trace_context text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  started_at timestamptz,
  finished_at timestamptz,
  PRIMARY KEY (job_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS jobs_idempotency ON public.jobs (org, idempotency_key) WHERE idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS jobs_one_active_per_org ON public.jobs (org) WHERE state IN ('dispatching','running','canceling');
CREATE INDEX IF NOT EXISTS jobs_claimable ON public.jobs (state, not_before, priority, job_id);
