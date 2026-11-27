SELECT id, org_id, dataset_id, merge_preview_id, job_id, fingerprint, status, preimage, detail, created_at, updated_at
FROM public.merge_applies
WHERE org_id = $org_id AND dataset_id = $dataset_id AND merge_preview_id = $merge_preview_id
