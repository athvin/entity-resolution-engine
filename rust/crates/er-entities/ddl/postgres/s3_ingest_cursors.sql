CREATE TABLE IF NOT EXISTS public.s3_ingest_cursors (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  source text NOT NULL,
  object_key text NOT NULL,
  etag text,
  size_bytes bigint NOT NULL,
  delivery_id text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (org_id, source)
);
CREATE UNIQUE INDEX IF NOT EXISTS s3_ingest_cursors_org_source ON public.s3_ingest_cursors (org_id, source);
