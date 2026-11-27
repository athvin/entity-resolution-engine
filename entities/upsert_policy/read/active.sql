SELECT DISTINCT ON (salesforce_connection_id, object)
       id, org_id, dataset_id, salesforce_connection_id, object, external_id_field, field_mapping, writeback_allowlist, sources, created_by, created_at, updated_at
FROM public.upsert_policies
WHERE org_id = $org_id AND dataset_id = $dataset_id
ORDER BY salesforce_connection_id, object, id DESC
