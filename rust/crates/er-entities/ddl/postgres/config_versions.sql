CREATE TABLE IF NOT EXISTS public.config_versions (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  version bigint NOT NULL,
  yaml text NOT NULL,
  config_hash text NOT NULL,
  state text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft', 'published')),
  tier text,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  published_at timestamptz,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (org_id, version)
);
CREATE TABLE IF NOT EXISTS public.config_versions_history (
  history_id bigserial NOT NULL,
  row_id text NOT NULL,
  op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  changed_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  old_row jsonb,
  new_row jsonb,
  PRIMARY KEY (history_id)
);
CREATE INDEX IF NOT EXISTS config_versions_history_row_id ON public.config_versions_history (row_id);
CREATE INDEX IF NOT EXISTS config_versions_history_changed_at ON public.config_versions_history (changed_at);
CREATE OR REPLACE TRIGGER er_history_config_versions
  AFTER INSERT OR UPDATE OR DELETE ON public.config_versions
  FOR EACH ROW EXECUTE FUNCTION public.er_history('config_versions_history');
