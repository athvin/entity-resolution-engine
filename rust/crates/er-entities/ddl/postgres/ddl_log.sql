CREATE TABLE IF NOT EXISTS public.ddl_log (
  id bigserial NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  phase text NOT NULL CHECK (phase IN ('ddl_command_end', 'sql_drop')),
  ddl_tag text NOT NULL,
  object_type text,
  object_identity text,
  command_text text,
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS ddl_log_created_at ON public.ddl_log (created_at);
