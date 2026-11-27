UPDATE public.merge_apply_operations
SET status = $to_status, outcome = $outcome, updated_at = now()
WHERE merge_apply_id = $merge_apply_id AND seq = $seq AND org_id = $org_id AND status = 'in_flight'
RETURNING id, org_id, dataset_id, merge_apply_id, seq, kind, payload, status, outcome, created_at, updated_at
