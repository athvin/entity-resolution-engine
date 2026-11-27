UPDATE public.merge_restore_steps
SET status = $to_status, outcome = $outcome, new_id = $new_id, updated_at = now()
WHERE merge_restore_id = $merge_restore_id AND seq = $seq AND org_id = $org_id AND status = 'in_flight'
RETURNING id, org_id, dataset_id, merge_restore_id, seq, kind, target_id, status, outcome, new_id, created_at, updated_at
