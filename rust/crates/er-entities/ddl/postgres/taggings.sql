CREATE TABLE IF NOT EXISTS public.taggings (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  tag_id text NOT NULL REFERENCES tags(id),
  subject_type text NOT NULL,
  subject_id text NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS taggings_org_tag ON public.taggings (org_id, tag_id);
CREATE INDEX IF NOT EXISTS taggings_org_subject ON public.taggings (org_id, subject_type, subject_id);
CREATE UNIQUE INDEX IF NOT EXISTS taggings_unique_application ON public.taggings (org_id, tag_id, subject_type, subject_id);
CREATE TABLE IF NOT EXISTS public.taggings_history (
  history_id bigserial NOT NULL,
  row_id text NOT NULL,
  op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  changed_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  old_row jsonb,
  new_row jsonb,
  PRIMARY KEY (history_id)
);
CREATE INDEX IF NOT EXISTS taggings_history_row_id ON public.taggings_history (row_id);
CREATE INDEX IF NOT EXISTS taggings_history_changed_at ON public.taggings_history (changed_at);
CREATE OR REPLACE TRIGGER er_history_taggings
  AFTER INSERT OR UPDATE OR DELETE ON public.taggings
  FOR EACH ROW EXECUTE FUNCTION public.er_history('taggings_history');
