CREATE TABLE IF NOT EXISTS public.audit_log (
  id bigserial NOT NULL,
  at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  org text,
  action text NOT NULL,
  detail jsonb NOT NULL DEFAULT '{}'::jsonb,
  PRIMARY KEY (id)
);
