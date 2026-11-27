SELECT id, org_id, dataset_id, upsert_preview_id, job_id, fingerprint, status, call_state, sf_record_id, preimage, detail, created_at, updated_at
FROM public.upsert_applies
WHERE org_id = $org_id AND dataset_id = $dataset_id AND status = 'applying'
ORDER BY id
LIMIT $limit
