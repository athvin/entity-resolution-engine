CREATE TABLE IF NOT EXISTS public.datasets (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  name text NOT NULL,
  display_name text,
  entity_type text NOT NULL DEFAULT 'person' CHECK (entity_type IN ('person', 'account')),
  config_path text NOT NULL,
  env jsonb NOT NULL DEFAULT '{}'::jsonb,
  drop_root text,
  s3_ingest_bucket text,
  s3_ingest_prefix text,
  active_config_version bigint,
  state text NOT NULL DEFAULT 'active' CHECK (state IN ('provisioning', 'active', 'suspended', 'purging')),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (org_id, name)
);
CREATE INDEX IF NOT EXISTS datasets_org_state ON public.datasets (org_id, state);
CREATE TABLE IF NOT EXISTS public.datasets_history (
  history_id bigserial NOT NULL,
  row_id text NOT NULL,
  op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  changed_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  old_row jsonb,
  new_row jsonb,
  PRIMARY KEY (history_id)
);
CREATE INDEX IF NOT EXISTS datasets_history_row_id ON public.datasets_history (row_id);
CREATE INDEX IF NOT EXISTS datasets_history_changed_at ON public.datasets_history (changed_at);
CREATE OR REPLACE TRIGGER er_history_datasets
  AFTER INSERT OR UPDATE OR DELETE ON public.datasets
  FOR EACH ROW EXECUTE FUNCTION public.er_history('datasets_history');
