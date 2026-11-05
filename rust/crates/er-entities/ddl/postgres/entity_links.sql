CREATE TABLE IF NOT EXISTS public.entity_links (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  parent_entity_id text NOT NULL,
  child_entity_id text NOT NULL,
  kind text NOT NULL CHECK (kind IN ('household', 'company', 'related')),
  origin text NOT NULL DEFAULT 'manual' CHECK (origin IN ('manual', 'rule', 'llm')),
  note text,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (dataset_id, parent_entity_id, child_entity_id, kind)
);
CREATE UNIQUE INDEX IF NOT EXISTS entity_links_org_pair_kind ON public.entity_links (org_id, parent_entity_id, child_entity_id, kind);
CREATE INDEX IF NOT EXISTS entity_links_org_child ON public.entity_links (org_id, child_entity_id);
CREATE INDEX IF NOT EXISTS entity_links_dataset ON public.entity_links (dataset_id, child_entity_id);
CREATE TABLE IF NOT EXISTS public.entity_links_history (
  history_id bigserial NOT NULL,
  row_id text NOT NULL,
  op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),
  changed_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  old_row jsonb,
  new_row jsonb,
  PRIMARY KEY (history_id)
);
CREATE INDEX IF NOT EXISTS entity_links_history_row_id ON public.entity_links_history (row_id);
CREATE INDEX IF NOT EXISTS entity_links_history_changed_at ON public.entity_links_history (changed_at);
CREATE OR REPLACE TRIGGER er_history_entity_links
  AFTER INSERT OR UPDATE OR DELETE ON public.entity_links
  FOR EACH ROW EXECUTE FUNCTION public.er_history('entity_links_history');
