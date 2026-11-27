CREATE TABLE IF NOT EXISTS public.merge_applies (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  merge_preview_id text NOT NULL REFERENCES merge_previews(id),
  job_id text NOT NULL,
  fingerprint text NOT NULL,
  status text NOT NULL DEFAULT 'applying' CHECK (status IN ('applying', 'applied', 'failed', 'skipped_stale', 'applied_externally')),
  preimage text NOT NULL DEFAULT '',
  detail text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE UNIQUE INDEX IF NOT EXISTS merge_applies_preview ON public.merge_applies (merge_preview_id);
CREATE INDEX IF NOT EXISTS merge_applies_dataset ON public.merge_applies (org_id, dataset_id, status);
