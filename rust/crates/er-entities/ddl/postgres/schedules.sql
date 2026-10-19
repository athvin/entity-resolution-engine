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
CREATE TABLE IF NOT EXISTS public.schedules_history (
  history_id bigserial NOT NULL,
  row_id text NOT NULL,
  op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  changed_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  old_row jsonb,
  new_row jsonb,
  PRIMARY KEY (history_id)
);
CREATE INDEX IF NOT EXISTS schedules_history_row_id ON public.schedules_history (row_id);
CREATE INDEX IF NOT EXISTS schedules_history_changed_at ON public.schedules_history (changed_at);
CREATE OR REPLACE TRIGGER er_history_schedules
  AFTER INSERT OR UPDATE OR DELETE ON public.schedules
  FOR EACH ROW EXECUTE FUNCTION public.er_history('schedules_history');
