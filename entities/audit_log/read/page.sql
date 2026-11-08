SELECT
  id,
  at,
  actor,
  org,
  dataset,
  action,
  trace_id,
  detail,
  created_at,
  updated_at
FROM public.audit_log
WHERE ($org::text IS NULL OR org = $org)
  AND ($action::text IS NULL OR action = $action)
  AND ($actor::text IS NULL OR actor = $actor)
  AND ($since::text IS NULL OR at >= $since::timestamptz)
  AND ($until::text IS NULL OR at < $until::timestamptz)
  AND ($before_id::bigint IS NULL OR id < $before_id)
ORDER BY id DESC
LIMIT $limit
