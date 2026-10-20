CREATE TABLE IF NOT EXISTS public.matching_models (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  name text NOT NULL,
  version bigint NOT NULL,
  entity_type text NOT NULL DEFAULT 'person' CHECK (entity_type IN ('person', 'account')),
  model_json text NOT NULL,
  model_hash text NOT NULL,
  model_version_label text NOT NULL,
  tf_snapshot_id text,
  state text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft', 'active', 'archived')),
  provenance jsonb NOT NULL DEFAULT '{}'::jsonb,
  created_by text NOT NULL,
  activated_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (org_id, name, version)
);
CREATE UNIQUE INDEX IF NOT EXISTS matching_models_org_name_version ON public.matching_models (org_id, name, version);
CREATE INDEX IF NOT EXISTS matching_models_org_state ON public.matching_models (org_id, state);
CREATE TABLE IF NOT EXISTS public.matching_models_history (
  history_id bigserial NOT NULL,
  row_id text NOT NULL,
  op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  changed_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  old_row jsonb,
  new_row jsonb,
  PRIMARY KEY (history_id)
);
CREATE INDEX IF NOT EXISTS matching_models_history_row_id ON public.matching_models_history (row_id);
CREATE INDEX IF NOT EXISTS matching_models_history_changed_at ON public.matching_models_history (changed_at);
CREATE OR REPLACE TRIGGER er_history_matching_models
  AFTER INSERT OR UPDATE OR DELETE ON public.matching_models
  FOR EACH ROW EXECUTE FUNCTION public.er_history('matching_models_history');
