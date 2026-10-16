CREATE TABLE IF NOT EXISTS public.config_versions (
  org text NOT NULL REFERENCES orgs(name),
  version bigint NOT NULL,
  yaml text NOT NULL,
  config_hash text NOT NULL,
  state text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft', 'published')),
  tier text,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  published_at timestamptz,
  PRIMARY KEY (org, version)
);
