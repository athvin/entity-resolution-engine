CREATE TABLE IF NOT EXISTS public.webhooks (
  webhook_id text NOT NULL,
  org text NOT NULL REFERENCES orgs(name),
  url text NOT NULL,
  events jsonb NOT NULL DEFAULT '[]'::jsonb,
  secret text NOT NULL,
  active boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (webhook_id)
);
