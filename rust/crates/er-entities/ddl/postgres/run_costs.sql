CREATE TABLE IF NOT EXISTS public.run_costs (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text,
  job_id text NOT NULL,
  task_instance_id text NOT NULL,
  task_id text NOT NULL,
  resource_class text,
  node_name text NOT NULL,
  attempt_state text NOT NULL,
  billing_period text NOT NULL,
  wait_seconds double precision NOT NULL,
  run_seconds double precision NOT NULL,
  allocated_usd double precision NOT NULL,
  billed_usd double precision,
  billed_as_of timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (task_instance_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS run_costs_attempt ON public.run_costs (task_instance_id);
CREATE INDEX IF NOT EXISTS run_costs_org_period ON public.run_costs (org_id, billing_period, dataset_id);
