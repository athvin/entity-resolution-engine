CREATE TABLE IF NOT EXISTS public.merge_restore_steps (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  merge_restore_id text NOT NULL REFERENCES merge_restores(id),
  seq bigint NOT NULL,
  kind text NOT NULL CHECK (kind IN ('never_assert', 'undelete', 'recreate', 'revert_master')),
  target_id text NOT NULL,
  status text NOT NULL DEFAULT 'in_flight' CHECK (status IN ('in_flight', 'done', 'failed')),
  outcome text,
  new_id text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE UNIQUE INDEX IF NOT EXISTS merge_restore_steps_seq ON public.merge_restore_steps (merge_restore_id, seq);
CREATE INDEX IF NOT EXISTS merge_restore_steps_org ON public.merge_restore_steps (org_id, dataset_id);
