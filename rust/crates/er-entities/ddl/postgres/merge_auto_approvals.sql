CREATE TABLE IF NOT EXISTS public.merge_auto_approvals (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  salesforce_connection_id text NOT NULL REFERENCES salesforce_connections(id),
  object text NOT NULL CHECK (object IN ('Lead', 'Contact', 'Account')),
  enabled boolean NOT NULL,
  min_score double precision NOT NULL,
  max_members bigint NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS merge_auto_approvals_scope ON public.merge_auto_approvals (org_id, dataset_id, salesforce_connection_id, object);
