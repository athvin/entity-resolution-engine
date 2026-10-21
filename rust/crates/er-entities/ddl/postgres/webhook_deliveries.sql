CREATE TABLE IF NOT EXISTS public.webhook_deliveries (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  webhook_id text NOT NULL REFERENCES webhooks(id),
  event_type text NOT NULL,
  payload jsonb NOT NULL,
  state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'delivered', 'failed')),
  attempt bigint NOT NULL DEFAULT 0,
  max_attempts bigint NOT NULL DEFAULT 6,
  next_attempt_at timestamptz,
  response_code bigint,
  error text,
  delivered_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS webhook_deliveries_org_recent ON public.webhook_deliveries (org_id, created_at);
CREATE INDEX IF NOT EXISTS webhook_deliveries_claimable ON public.webhook_deliveries (state, next_attempt_at);
