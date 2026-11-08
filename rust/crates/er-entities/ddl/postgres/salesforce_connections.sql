CREATE TABLE IF NOT EXISTS public.salesforce_connections (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  name text NOT NULL,
  instance_url text NOT NULL,
  auth_kind text NOT NULL,
  client_id text NOT NULL,
  client_secret text NOT NULL,
  api_version text NOT NULL,
  active boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (org_id, name)
);
CREATE UNIQUE INDEX IF NOT EXISTS salesforce_connections_org_name ON public.salesforce_connections (org_id, name);
