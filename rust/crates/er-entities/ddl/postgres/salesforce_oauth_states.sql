CREATE TABLE IF NOT EXISTS public.salesforce_oauth_states (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  salesforce_connection_id text NOT NULL REFERENCES salesforce_connections(id),
  state_sha256 text NOT NULL,
  code_verifier text NOT NULL,
  redirect_uri text NOT NULL,
  allow_automatic boolean NOT NULL,
  requested_by text NOT NULL,
  expires_at timestamptz NOT NULL,
  consumed_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE UNIQUE INDEX IF NOT EXISTS salesforce_oauth_states_state ON public.salesforce_oauth_states (state_sha256);
CREATE INDEX IF NOT EXISTS salesforce_oauth_states_org ON public.salesforce_oauth_states (org_id, salesforce_connection_id);
