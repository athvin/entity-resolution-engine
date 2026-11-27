CREATE TABLE IF NOT EXISTS public.writeback_policies (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  salesforce_connection_id text NOT NULL REFERENCES salesforce_connections(id),
  object text NOT NULL CHECK (object IN ('Lead', 'Contact', 'Account')),
  election_policy text NOT NULL,
  writeback_allowlist jsonb NOT NULL,
  field_mapping jsonb NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS writeback_policies_scope ON public.writeback_policies (dataset_id, salesforce_connection_id, object, id);
CREATE INDEX IF NOT EXISTS writeback_policies_org ON public.writeback_policies (org_id, dataset_id);
