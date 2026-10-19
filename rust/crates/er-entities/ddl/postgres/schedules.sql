CREATE TABLE IF NOT EXISTS public.schedules (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  kind text NOT NULL,
  params jsonb NOT NULL DEFAULT '{}'::jsonb,
  cron text NOT NULL,
  enabled boolean NOT NULL DEFAULT true,
  source text NOT NULL DEFAULT 'api',
  last_enqueued_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE UNIQUE INDEX IF NOT EXISTS schedules_one_correction_per_org ON public.schedules (org_id) WHERE source = 'config:correction_pass';
CREATE UNIQUE INDEX IF NOT EXISTS schedules_one_maintenance_per_org ON public.schedules (org_id) WHERE source = 'system:lake_maintain';
