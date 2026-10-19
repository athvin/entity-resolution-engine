CREATE TABLE IF NOT EXISTS public.api_keys (
  key_id text NOT NULL,
  org text NOT NULL REFERENCES orgs(name),
  role text NOT NULL CHECK (role IN ('admin', 'steward', 'viewer')),
  key_hash text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  revoked_at timestamptz,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (key_id)
);
