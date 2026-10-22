CREATE TABLE IF NOT EXISTS public.tenant_schema_states (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  catalog_hash text NOT NULL,
  status text NOT NULL CHECK (status IN ('current', 'migrated', 'breaking', 'error')),
  detail text,
  checked_at timestamptz NOT NULL DEFAULT now(),
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (org_id)
);
CREATE INDEX IF NOT EXISTS tenant_schema_states_org ON public.tenant_schema_states (org_id);
