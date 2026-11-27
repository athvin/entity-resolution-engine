INSERT INTO public.source_delivery_provenance
  (id, org_id, dataset_id, run_id, delivery_id, source, salesforce_connection_id, object,
   delivery_sha256, source_sha256)
VALUES
  ($id, $org_id, $dataset_id, $run_id, $delivery_id, $source, $salesforce_connection_id, $object,
   $delivery_sha256, $source_sha256)
ON CONFLICT DO NOTHING
