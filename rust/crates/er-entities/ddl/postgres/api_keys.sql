CREATE TABLE IF NOT EXISTS public.api_keys (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  role text NOT NULL CHECK (role IN ('admin', 'steward', 'viewer')),
  key_hash text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  revoked_at timestamptz,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE TABLE IF NOT EXISTS public.api_keys_history (
  history_id bigserial NOT NULL,
  row_id text NOT NULL,
  op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  changed_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  old_row jsonb,
  new_row jsonb,
  PRIMARY KEY (history_id)
);
CREATE INDEX IF NOT EXISTS api_keys_history_row_id ON public.api_keys_history (row_id);
CREATE INDEX IF NOT EXISTS api_keys_history_changed_at ON public.api_keys_history (changed_at);
CREATE OR REPLACE TRIGGER er_history_api_keys
  AFTER INSERT OR UPDATE OR DELETE ON public.api_keys
  FOR EACH ROW EXECUTE FUNCTION public.er_history('api_keys_history');
