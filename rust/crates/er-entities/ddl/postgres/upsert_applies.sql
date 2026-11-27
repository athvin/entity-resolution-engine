CREATE TABLE IF NOT EXISTS public.upsert_applies (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  upsert_preview_id text NOT NULL REFERENCES upsert_previews(id),
  job_id text NOT NULL,
  fingerprint text NOT NULL,
  status text NOT NULL DEFAULT 'applying' CHECK (status IN ('applying', 'created', 'updated', 'failed', 'skipped_stale')),
  call_state text NOT NULL DEFAULT 'none' CHECK (call_state IN ('none', 'in_flight')),
  sf_record_id text,
  preimage text NOT NULL DEFAULT '',
  detail text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE UNIQUE INDEX IF NOT EXISTS upsert_applies_preview ON public.upsert_applies (upsert_preview_id);
CREATE INDEX IF NOT EXISTS upsert_applies_dataset ON public.upsert_applies (org_id, dataset_id, status);
