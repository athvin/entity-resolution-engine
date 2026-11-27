CREATE TABLE IF NOT EXISTS public.upsert_previews (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  salesforce_connection_id text NOT NULL REFERENCES salesforce_connections(id),
  object text NOT NULL CHECK (object IN ('Lead', 'Contact', 'Account')),
  entity_id text NOT NULL,
  external_id_field text NOT NULL,
  external_id_value text NOT NULL,
  anchor_record_key text NOT NULL,
  fields jsonb NOT NULL,
  fingerprint text NOT NULL,
  lake_snapshot bigint NOT NULL,
  origin_run_id text NOT NULL,
  upsert_policy_id text NOT NULL REFERENCES upsert_policies(id),
  status text NOT NULL DEFAULT 'planned' CHECK (status IN ('planned', 'approved', 'rejected', 'superseded')),
  decided_by text,
  decided_at timestamptz,
  decision_reason text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE UNIQUE INDEX IF NOT EXISTS upsert_previews_fingerprint ON public.upsert_previews (dataset_id, fingerprint);
CREATE INDEX IF NOT EXISTS upsert_previews_dataset ON public.upsert_previews (org_id, dataset_id, status, id);
CREATE INDEX IF NOT EXISTS upsert_previews_entity ON public.upsert_previews (dataset_id, entity_id, salesforce_connection_id, object);
