CREATE TABLE IF NOT EXISTS public.merge_previews (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  salesforce_connection_id text NOT NULL REFERENCES salesforce_connections(id),
  object text NOT NULL CHECK (object IN ('Lead', 'Contact', 'Account')),
  entity_id text NOT NULL,
  fingerprint text NOT NULL,
  format_version bigint NOT NULL,
  lake_snapshot bigint NOT NULL,
  origin_run_id text NOT NULL,
  writeback_policy_id text NOT NULL REFERENCES writeback_policies(id),
  master_record_key text NOT NULL,
  member_count bigint NOT NULL,
  score double precision,
  proposal jsonb NOT NULL,
  field_changes jsonb NOT NULL,
  related_counts jsonb,
  related_counts_state text NOT NULL CHECK (related_counts_state IN ('unavailable', 'known')),
  status text NOT NULL DEFAULT 'planned' CHECK (status IN ('planned', 'approved', 'rejected', 'superseded')),
  decided_by text,
  decided_at timestamptz,
  decision_reason text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (dataset_id, fingerprint)
);
CREATE UNIQUE INDEX IF NOT EXISTS merge_previews_fingerprint ON public.merge_previews (dataset_id, fingerprint);
CREATE INDEX IF NOT EXISTS merge_previews_status ON public.merge_previews (dataset_id, status, id);
CREATE INDEX IF NOT EXISTS merge_previews_group ON public.merge_previews (dataset_id, salesforce_connection_id, object, entity_id);
CREATE INDEX IF NOT EXISTS merge_previews_org ON public.merge_previews (org_id, dataset_id);
