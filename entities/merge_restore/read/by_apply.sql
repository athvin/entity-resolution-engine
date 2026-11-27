SELECT id, org_id, dataset_id, merge_apply_id, merge_preview_id, requested_by, status, conflicts, detail, created_at, updated_at
FROM public.merge_restores
WHERE org_id = $org_id AND dataset_id = $dataset_id AND merge_apply_id = $merge_apply_id
