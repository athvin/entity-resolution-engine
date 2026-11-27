INSERT INTO public.merge_restores (id, org_id, dataset_id, merge_apply_id, merge_preview_id, requested_by)
VALUES ($id, $org_id, $dataset_id, $merge_apply_id, $merge_preview_id, $requested_by)
ON CONFLICT (merge_apply_id) DO NOTHING
