CREATE TABLE IF NOT EXISTS public.fixups (
  id text NOT NULL,
  name text NOT NULL,
  applied_by text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (name)
);
