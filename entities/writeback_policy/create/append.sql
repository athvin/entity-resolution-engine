INSERT INTO public.writeback_policies
  (id, org_id, dataset_id, salesforce_connection_id, object, election_policy,
   writeback_allowlist, field_mapping, created_by)
VALUES
  ($id, $org_id, $dataset_id, $salesforce_connection_id, $object, $election_policy,
   $writeback_allowlist, $field_mapping, $created_by)
RETURNING id, org_id, dataset_id, salesforce_connection_id, object, election_policy,
          writeback_allowlist, field_mapping, created_by, created_at, updated_at
