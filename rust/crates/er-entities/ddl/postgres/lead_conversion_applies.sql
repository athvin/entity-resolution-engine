CREATE TABLE IF NOT EXISTS public.lead_conversion_applies (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  lead_conversion_id text NOT NULL REFERENCES lead_conversions(id),
  job_id text NOT NULL,
  fingerprint text NOT NULL,
  status text NOT NULL DEFAULT 'applying' CHECK (status IN ('applying', 'converted', 'converted_externally', 'failed', 'skipped_stale')),
  call_state text NOT NULL DEFAULT 'none' CHECK (call_state IN ('none', 'in_flight')),
  account_id text,
  detail text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE UNIQUE INDEX IF NOT EXISTS lead_conversion_applies_proposal ON public.lead_conversion_applies (lead_conversion_id);
CREATE INDEX IF NOT EXISTS lead_conversion_applies_dataset ON public.lead_conversion_applies (org_id, dataset_id, status);
