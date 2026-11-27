SELECT id, org_id, dataset_id, run_id, delivery_id, source, salesforce_connection_id, object,
       delivery_sha256, source_sha256, created_at, updated_at
FROM public.source_delivery_provenance
WHERE org_id = $org_id AND dataset_id = $dataset_id
ORDER BY run_id
LIMIT 100000
