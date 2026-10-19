SELECT
  action_id,
  org,
  action,
  state,
  error,
  created_by,
  created_at,
  applied_at,
  updated_at
FROM public.staged_steward_actions
WHERE org = $org AND state = 'pending'
ORDER BY created_at, action_id
