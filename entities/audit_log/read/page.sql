SELECT
  id,
  at,
  actor,
  org,
  action,
  detail,
  created_at,
  updated_at
FROM public.audit_log
WHERE ($org::text IS NULL OR org = $org)
  AND ($action::text IS NULL OR action = $action)
  AND ($before_id::bigint IS NULL OR id < $before_id)
ORDER BY id DESC
LIMIT $limit
