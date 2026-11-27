SELECT id, org_id, dataset_id, lead_conversion_id, job_id, fingerprint, status, call_state, account_id, detail, created_at, updated_at
FROM public.lead_conversion_applies
WHERE org_id = $org_id AND dataset_id = $dataset_id AND status = 'applying'
ORDER BY id
LIMIT $limit
