UPDATE public.merge_applies
SET status = $to_status, detail = $detail, updated_at = now()
WHERE id = $id AND org_id = $org_id AND status = 'applying'
RETURNING id, org_id, dataset_id, merge_preview_id, job_id, fingerprint, status, preimage, detail, created_at, updated_at
