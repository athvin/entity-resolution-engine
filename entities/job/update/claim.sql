UPDATE public.jobs
SET state = 'dispatching', started_at = now(), updated_at = now(),
    -- Count the attempt as it is handed out: a worker that dies without
    -- reporting has still used one, which is what stops an infinite retry.
    attempt = jobs.attempt + 1
WHERE id = (
  SELECT j.id FROM public.jobs j
  WHERE j.state = 'queued' AND (j.not_before IS NULL OR j.not_before <= now())
    -- The writer lock, per DATASET (each dataset is its own lake). An
    -- org-level job (dataset_id IS NULL: provision, a pre-dataset row)
    -- conflicts with ANY active job of its org — it reconciles the org's
    -- lakes, so nothing of the org's may run beside it, and vice versa.
    AND NOT EXISTS (
      SELECT 1 FROM public.jobs a
      WHERE a.state IN ('dispatching', 'running', 'canceling')
        AND ((j.dataset_id IS NOT NULL
                AND (a.dataset_id = j.dataset_id
                       OR (a.org_id = j.org_id AND a.dataset_id IS NULL)))
          OR (j.dataset_id IS NULL AND a.org_id = j.org_id)))
    -- The noisy-neighbor cap: at most N active jobs per org across its
    -- datasets. A COUNT cannot be index-enforced, so the claim transaction
    -- holds an advisory lock (pg.rs) and the per-dataset partial unique above
    -- stays the database-level backstop.
    AND (SELECT count(*) FROM public.jobs c
         WHERE c.org_id = j.org_id
           AND c.state IN ('dispatching', 'running', 'canceling'))
        < coalesce((SELECT o.max_concurrent_jobs FROM public.orgs o
                    WHERE o.id = j.org_id), $default_cap)
  ORDER BY j.priority DESC, j.id
  FOR UPDATE SKIP LOCKED
  LIMIT 1)
RETURNING
  id,
  org_id,
  dataset_id,
  kind,
  params,
  state,
  priority,
  idempotency_key,
  run_id,
  attempt,
  max_attempts,
  error_class,
  error_detail,
  not_before,
  created_by,
  schedule_id,
  resource_class,
  trace_context,
  structural_fingerprint,
  policy_hash,
  image_digest,
  clear_number,
  owner_epoch,
  created_at,
  updated_at,
  started_at,
  finished_at
