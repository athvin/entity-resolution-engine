SELECT
  id,
  org_id, dataset_id,
  action,
  state,
  error,
  created_by,
  created_at,
  applied_at,
  updated_at
FROM public.staged_steward_actions
WHERE dataset_id = $dataset_id AND state = 'pending'
ORDER BY created_at, id
