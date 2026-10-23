CREATE TABLE IF NOT EXISTS public.tags (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  name text NOT NULL,
  description text,
  active boolean NOT NULL DEFAULT true,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS tags_org_name ON public.tags (org_id, name);
CREATE UNIQUE INDEX IF NOT EXISTS tags_one_active_per_name ON public.tags (org_id, name) WHERE active;
CREATE TABLE IF NOT EXISTS public.tags_history (
  history_id bigserial NOT NULL,
  row_id text NOT NULL,
  op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  changed_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  old_row jsonb,
  new_row jsonb,
  PRIMARY KEY (history_id)
);
CREATE INDEX IF NOT EXISTS tags_history_row_id ON public.tags_history (row_id);
CREATE INDEX IF NOT EXISTS tags_history_changed_at ON public.tags_history (changed_at);
CREATE OR REPLACE TRIGGER er_history_tags
  AFTER INSERT OR UPDATE OR DELETE ON public.tags
  FOR EACH ROW EXECUTE FUNCTION public.er_history('tags_history');
