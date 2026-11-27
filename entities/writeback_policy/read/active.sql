SELECT DISTINCT ON (salesforce_connection_id, object)
       id, org_id, dataset_id, salesforce_connection_id, object, election_policy,
       writeback_allowlist, field_mapping, created_by, created_at, updated_at
FROM public.writeback_policies
WHERE org_id = $org_id AND dataset_id = $dataset_id
ORDER BY salesforce_connection_id, object, id DESC
