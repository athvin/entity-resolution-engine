INSERT INTO public.upsert_applies (id, org_id, dataset_id, upsert_preview_id, job_id, fingerprint)
SELECT $id, p.org_id, p.dataset_id, p.id, $job_id, p.fingerprint
FROM public.upsert_previews p
WHERE p.id = $upsert_preview_id AND p.org_id = $org_id AND p.dataset_id = $dataset_id
  AND p.fingerprint = $fingerprint AND p.status = $approved_status
ON CONFLICT (upsert_preview_id) DO NOTHING
