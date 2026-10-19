CREATE TABLE IF NOT EXISTS public.webhooks (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  url text NOT NULL,
  events jsonb NOT NULL DEFAULT '[]'::jsonb,
  secret text NOT NULL,
  active boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
