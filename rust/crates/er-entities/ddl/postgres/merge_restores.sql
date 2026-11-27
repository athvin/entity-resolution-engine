CREATE TABLE IF NOT EXISTS public.merge_restores (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  merge_apply_id text NOT NULL REFERENCES merge_applies(id),
  merge_preview_id text NOT NULL REFERENCES merge_previews(id),
  requested_by text NOT NULL,
  status text NOT NULL DEFAULT 'restoring' CHECK (status IN ('restoring', 'restored', 'partial', 'failed')),
  conflicts text NOT NULL DEFAULT '[]',
  detail text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE UNIQUE INDEX IF NOT EXISTS merge_restores_apply ON public.merge_restores (merge_apply_id);
CREATE INDEX IF NOT EXISTS merge_restores_dataset ON public.merge_restores (org_id, dataset_id, status);
