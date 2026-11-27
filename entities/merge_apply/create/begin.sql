INSERT INTO public.merge_applies (id, org_id, dataset_id, merge_preview_id, job_id, fingerprint)
SELECT $id, p.org_id, p.dataset_id, p.id, $job_id, p.fingerprint
FROM public.merge_previews p
WHERE p.id = $merge_preview_id AND p.org_id = $org_id AND p.dataset_id = $dataset_id
  AND p.fingerprint = $fingerprint AND p.status = $approved_status
ON CONFLICT (merge_preview_id) DO NOTHING
