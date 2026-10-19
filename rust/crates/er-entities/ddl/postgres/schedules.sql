CREATE TABLE IF NOT EXISTS public.schedules (
  schedule_id text NOT NULL,
  org text NOT NULL REFERENCES orgs(name),
  kind text NOT NULL,
  params jsonb NOT NULL DEFAULT '{}'::jsonb,
  cron text NOT NULL,
  enabled boolean NOT NULL DEFAULT true,
  source text NOT NULL DEFAULT 'api',
  last_enqueued_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (schedule_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS schedules_one_correction_per_org ON public.schedules (org) WHERE source = 'config:correction_pass';
CREATE UNIQUE INDEX IF NOT EXISTS schedules_one_maintenance_per_org ON public.schedules (org) WHERE source = 'system:lake_maintain';
