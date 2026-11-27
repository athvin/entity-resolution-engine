CREATE TABLE IF NOT EXISTS public.source_delivery_provenance (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  run_id text NOT NULL,
  delivery_id text NOT NULL,
  source text NOT NULL,
  salesforce_connection_id text NOT NULL REFERENCES salesforce_connections(id),
  object text NOT NULL,
  delivery_sha256 text NOT NULL,
  source_sha256 text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (dataset_id, run_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS source_delivery_provenance_run ON public.source_delivery_provenance (dataset_id, run_id);
CREATE UNIQUE INDEX IF NOT EXISTS source_delivery_provenance_delivery ON public.source_delivery_provenance (dataset_id, delivery_id);
CREATE INDEX IF NOT EXISTS source_delivery_provenance_source ON public.source_delivery_provenance (dataset_id, source);
CREATE INDEX IF NOT EXISTS source_delivery_provenance_org ON public.source_delivery_provenance (org_id, dataset_id);
