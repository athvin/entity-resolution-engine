CREATE TABLE IF NOT EXISTS public.staged_steward_actions (
  action_id text NOT NULL,
  org text NOT NULL REFERENCES orgs(name),
  action jsonb NOT NULL,
  state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'applied', 'failed')),
  error text,
  created_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  applied_at timestamptz,
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (action_id)
);
CREATE INDEX IF NOT EXISTS staged_pending ON public.staged_steward_actions (org, state);
