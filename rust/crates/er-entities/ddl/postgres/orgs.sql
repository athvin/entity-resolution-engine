CREATE TABLE IF NOT EXISTS public.orgs (
  name text NOT NULL,
  config_path text NOT NULL,
  env jsonb NOT NULL DEFAULT '{}'::jsonb,
  drop_root text,
  active_config_version bigint,
  state text NOT NULL DEFAULT 'active' CHECK (state IN ('provisioning', 'active', 'suspended', 'purging')),
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (name)
);
