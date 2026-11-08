CREATE TABLE IF NOT EXISTS public.audit_log (
  id bigserial NOT NULL,
  at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  org text,
  dataset text,
  action text NOT NULL,
  trace_id text,
  detail jsonb NOT NULL DEFAULT '{}'::jsonb,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS audit_log_at ON public.audit_log (at);
