CREATE TABLE IF NOT EXISTS public.salesforce_delegated_grants (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  salesforce_connection_id text NOT NULL REFERENCES salesforce_connections(id),
  sf_org_id text NOT NULL,
  sf_user_id text NOT NULL,
  sf_username text,
  refresh_token text NOT NULL,
  scopes text NOT NULL,
  allow_automatic boolean NOT NULL,
  status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked')),
  authorized_by text NOT NULL,
  revoked_reason text,
  revoked_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS salesforce_delegated_grants_connection ON public.salesforce_delegated_grants (org_id, salesforce_connection_id, status);
