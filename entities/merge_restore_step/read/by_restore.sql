SELECT id, org_id, dataset_id, merge_restore_id, seq, kind, target_id, status, outcome, new_id, created_at, updated_at
FROM public.merge_restore_steps
WHERE org_id = $org_id AND merge_restore_id = $merge_restore_id
ORDER BY seq
