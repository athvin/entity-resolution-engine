CREATE TABLE IF NOT EXISTS public.config_versions (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  version bigint NOT NULL,
  yaml text NOT NULL,
  config_hash text NOT NULL,
  state text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft', 'published')),
  tier text,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  published_at timestamptz,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (org_id, version)
);
