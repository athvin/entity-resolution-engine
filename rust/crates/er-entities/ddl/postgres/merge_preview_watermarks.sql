CREATE TABLE IF NOT EXISTS public.merge_preview_watermarks (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  run_snapshot_end bigint NOT NULL,
  run_id text NOT NULL,
  lake_snapshot bigint NOT NULL,
  job_id text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (dataset_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS merge_preview_watermarks_dataset ON public.merge_preview_watermarks (dataset_id);
CREATE INDEX IF NOT EXISTS merge_preview_watermarks_org ON public.merge_preview_watermarks (org_id, dataset_id);
