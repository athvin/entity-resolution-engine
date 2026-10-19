SELECT
  id,
  org_id,
  action,
  state,
  error,
  created_by,
  created_at,
  applied_at,
  updated_at
FROM public.staged_steward_actions
WHERE org_id = $org_id AND state = 'pending'
ORDER BY created_at, id
