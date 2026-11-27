UPDATE public.merge_restores
SET status = $to_status, conflicts = $conflicts, detail = $detail, updated_at = now()
WHERE id = $id AND org_id = $org_id AND status = 'restoring'
RETURNING id, org_id, dataset_id, merge_apply_id, merge_preview_id, requested_by, status, conflicts, detail, created_at, updated_at
