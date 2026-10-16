SELECT
  job_id,
  org,
  kind,
  params,
  state,
  priority,
  idempotency_key,
  run_id,
  attempt,
  max_attempts,
  not_before,
  created_by,
  schedule_id,
  resource_class,
  trace_context,
  created_at,
  updated_at,
  started_at,
  finished_at
FROM public.jobs
-- The `::text` casts let Postgres infer the optional-filter params' type (an
-- `$x IS NULL` use alone is ambiguous to its planner; DuckDB infers it, Postgres
-- does not). Casts are planned by DataFusion unchanged.
WHERE ($org::text IS NULL OR org = $org)
  AND ($state::text IS NULL OR state = $state)
ORDER BY job_id DESC
LIMIT $limit
