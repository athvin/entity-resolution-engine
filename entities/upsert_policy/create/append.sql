INSERT INTO public.upsert_policies
  (id, org_id, dataset_id, salesforce_connection_id, object, external_id_field, field_mapping,
   writeback_allowlist, sources, created_by)
VALUES
  ($id, $org_id, $dataset_id, $salesforce_connection_id, $object, $external_id_field, $field_mapping,
   $writeback_allowlist, $sources, $created_by)
RETURNING id, org_id, dataset_id, salesforce_connection_id, object, external_id_field, field_mapping, writeback_allowlist, sources, created_by, created_at, updated_at
