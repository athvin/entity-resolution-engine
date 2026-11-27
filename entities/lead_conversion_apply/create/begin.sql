INSERT INTO public.lead_conversion_applies (id, org_id, dataset_id, lead_conversion_id, job_id, fingerprint)
SELECT $id, p.org_id, p.dataset_id, p.id, $job_id, p.fingerprint
FROM public.lead_conversions p
WHERE p.id = $lead_conversion_id AND p.org_id = $org_id AND p.dataset_id = $dataset_id
  AND p.fingerprint = $fingerprint AND p.status = $approved_status
ON CONFLICT (lead_conversion_id) DO NOTHING
