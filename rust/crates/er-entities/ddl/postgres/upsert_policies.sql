CREATE TABLE IF NOT EXISTS public.upsert_policies (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  salesforce_connection_id text NOT NULL REFERENCES salesforce_connections(id),
  object text NOT NULL CHECK (object IN ('Lead', 'Contact', 'Account')),
  external_id_field text NOT NULL,
  field_mapping jsonb NOT NULL,
  writeback_allowlist jsonb NOT NULL,
  sources jsonb NOT NULL,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS upsert_policies_scope ON public.upsert_policies (org_id, dataset_id, salesforce_connection_id, object);
