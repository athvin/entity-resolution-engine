CREATE TABLE IF NOT EXISTS public.webhooks (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  url text NOT NULL,
  events jsonb NOT NULL DEFAULT '[]'::jsonb,
  secret text NOT NULL,
  active boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE TABLE IF NOT EXISTS public.webhooks_history (
  history_id bigserial NOT NULL,
  row_id text NOT NULL,
  op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  changed_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  old_row jsonb,
  new_row jsonb,
  PRIMARY KEY (history_id)
);
CREATE INDEX IF NOT EXISTS webhooks_history_row_id ON public.webhooks_history (row_id);
CREATE INDEX IF NOT EXISTS webhooks_history_changed_at ON public.webhooks_history (changed_at);
CREATE OR REPLACE TRIGGER er_history_webhooks
  AFTER INSERT OR UPDATE OR DELETE ON public.webhooks
  FOR EACH ROW EXECUTE FUNCTION public.er_history('webhooks_history');
