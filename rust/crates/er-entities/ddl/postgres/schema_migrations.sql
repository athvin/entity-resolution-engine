CREATE TABLE IF NOT EXISTS public.schema_migrations (
  id text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  catalog_hash text NOT NULL,
  kind text NOT NULL CHECK (kind IN ('applied', 'verified')),
  relation text,
  statement text,
  applied_by text NOT NULL,
  PRIMARY KEY (id)
);
CREATE INDEX IF NOT EXISTS schema_migrations_created_at ON public.schema_migrations (created_at);
