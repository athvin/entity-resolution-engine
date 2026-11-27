UPDATE public.lead_conversion_applies
SET call_state = 'in_flight', account_id = $account_id, updated_at = now()
WHERE id = $id AND org_id = $org_id AND status = 'applying'
RETURNING id, org_id, dataset_id, lead_conversion_id, job_id, fingerprint, status, call_state, account_id, detail, created_at, updated_at
