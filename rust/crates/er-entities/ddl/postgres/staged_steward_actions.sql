CREATE TABLE IF NOT EXISTS public.staged_steward_actions (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  action jsonb NOT NULL,
  state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'applied', 'failed')),
  error text,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  applied_at timestamptz,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS staged_pending ON public.staged_steward_actions (org_id, state);
