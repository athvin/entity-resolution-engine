CREATE TABLE IF NOT EXISTS public.merge_preview_journal (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  merge_preview_id text NOT NULL REFERENCES merge_previews(id),
  from_status text,
  to_status text NOT NULL,
  fingerprint text NOT NULL,
  actor text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS merge_preview_journal_preview ON public.merge_preview_journal (dataset_id, merge_preview_id, id);
CREATE INDEX IF NOT EXISTS merge_preview_journal_org ON public.merge_preview_journal (org_id, dataset_id);
