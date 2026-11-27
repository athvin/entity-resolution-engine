CREATE TABLE IF NOT EXISTS public.merge_apply_operations (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  merge_apply_id text NOT NULL REFERENCES merge_applies(id),
  seq bigint NOT NULL,
  kind text NOT NULL CHECK (kind IN ('update_master', 'merge')),
  payload text NOT NULL,
  status text NOT NULL DEFAULT 'in_flight' CHECK (status IN ('in_flight', 'done', 'failed')),
  outcome text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE UNIQUE INDEX IF NOT EXISTS merge_apply_operations_seq ON public.merge_apply_operations (merge_apply_id, seq);
CREATE INDEX IF NOT EXISTS merge_apply_operations_org ON public.merge_apply_operations (org_id, dataset_id);
