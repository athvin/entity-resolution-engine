UPDATE public.upsert_applies
SET call_state = 'in_flight', preimage = $preimage, updated_at = now()
WHERE id = $id AND org_id = $org_id AND status = 'applying'
RETURNING id, org_id, dataset_id, upsert_preview_id, job_id, fingerprint, status, call_state, sf_record_id, preimage, detail, created_at, updated_at
