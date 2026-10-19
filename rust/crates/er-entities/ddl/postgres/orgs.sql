CREATE TABLE IF NOT EXISTS public.orgs (
  id text NOT NULL,
  name text NOT NULL,
  config_path text NOT NULL,
  env jsonb NOT NULL DEFAULT '{}'::jsonb,
  drop_root text,
  active_config_version bigint,
  state text NOT NULL DEFAULT 'active' CHECK (state IN ('provisioning', 'active', 'suspended', 'purging')),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (name)
);
CREATE TABLE IF NOT EXISTS public.orgs_history (
  history_id bigserial NOT NULL,
  row_id text NOT NULL,
  op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  changed_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  old_row jsonb,
  new_row jsonb,
  PRIMARY KEY (history_id)
);
CREATE INDEX IF NOT EXISTS orgs_history_row_id ON public.orgs_history (row_id);
CREATE INDEX IF NOT EXISTS orgs_history_changed_at ON public.orgs_history (changed_at);
CREATE OR REPLACE TRIGGER er_history_orgs
  AFTER INSERT OR UPDATE OR DELETE ON public.orgs
  FOR EACH ROW EXECUTE FUNCTION public.er_history('orgs_history');
