CREATE TABLE IF NOT EXISTS public.remote_attempts (
  id text NOT NULL,
  org_id text NOT NULL REFERENCES orgs(id),
  dataset_id text NOT NULL REFERENCES datasets(id),
  job_id text NOT NULL REFERENCES jobs(id),
  task_instance_id text NOT NULL REFERENCES task_instances(id),
  launch bigint NOT NULL,
  binding_generation bigint NOT NULL,
  namespace text NOT NULL,
  session_id text,
  request_id text,
  commit_request_id text,
  phase text NOT NULL CHECK (phase IN ('opening', 'active', 'committing', 'unknown', 'committed', 'rolled_back', 'not_executed')),
  released_at timestamptz,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (id),
  UNIQUE (dataset_id, task_instance_id, launch)
);
CREATE UNIQUE INDEX IF NOT EXISTS remote_attempts_dataset_unresolved ON public.remote_attempts (dataset_id) WHERE released_at IS NULL;
CREATE INDEX IF NOT EXISTS remote_attempts_org ON public.remote_attempts (org_id);
CREATE INDEX IF NOT EXISTS remote_attempts_dataset_history ON public.remote_attempts (dataset_id, created_at);
