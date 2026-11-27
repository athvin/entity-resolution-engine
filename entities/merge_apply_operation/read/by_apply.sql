SELECT id, org_id, dataset_id, merge_apply_id, seq, kind, payload, status, outcome, created_at, updated_at
FROM public.merge_apply_operations
WHERE org_id = $org_id AND merge_apply_id = $merge_apply_id
ORDER BY seq
