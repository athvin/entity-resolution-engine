CREATE TABLE IF NOT EXISTS public.lead_conversions (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  salesforce_connection_id text NOT NULL REFERENCES salesforce_connections(id),
  entity_id text NOT NULL,
  lead_external_id text NOT NULL,
  lead_record_key text NOT NULL,
  lead_modstamp text NOT NULL,
  contact_external_id text NOT NULL,
  contact_record_key text NOT NULL,
  contact_modstamp text NOT NULL,
  fingerprint text NOT NULL,
  lake_snapshot bigint NOT NULL,
  origin_run_id text NOT NULL,
  lead_writeback_policy_id text NOT NULL REFERENCES writeback_policies(id),
  contact_writeback_policy_id text NOT NULL REFERENCES writeback_policies(id),
  status text NOT NULL DEFAULT 'planned' CHECK (status IN ('planned', 'approved', 'rejected', 'superseded')),
  decided_by text,
  decided_at timestamptz,
  decision_reason text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE UNIQUE INDEX IF NOT EXISTS lead_conversions_fingerprint ON public.lead_conversions (dataset_id, fingerprint);
CREATE INDEX IF NOT EXISTS lead_conversions_dataset ON public.lead_conversions (org_id, dataset_id, status, id);
CREATE INDEX IF NOT EXISTS lead_conversions_lead ON public.lead_conversions (dataset_id, lead_external_id);
