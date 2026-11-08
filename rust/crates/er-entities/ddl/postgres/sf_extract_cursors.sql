CREATE TABLE IF NOT EXISTS public.sf_extract_cursors (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  source text NOT NULL,
  object text NOT NULL,
  watermark text,
  delivery_id text NOT NULL,
  rows_extracted bigint NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (dataset_id, source)
);
CREATE UNIQUE INDEX IF NOT EXISTS sf_extract_cursors_dataset_source ON public.sf_extract_cursors (dataset_id, source);
CREATE INDEX IF NOT EXISTS sf_extract_cursors_org_source ON public.sf_extract_cursors (org_id, source);
