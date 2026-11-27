INSERT INTO public.merge_previews
  (id, org_id, dataset_id, salesforce_connection_id, object, entity_id, fingerprint,
   format_version, lake_snapshot, origin_run_id, writeback_policy_id, master_record_key,
   member_count, proposal, field_changes, related_counts, related_counts_state)
VALUES
  ($id, $org_id, $dataset_id, $salesforce_connection_id, $object, $entity_id, $fingerprint,
   $format_version, $lake_snapshot, $origin_run_id, $writeback_policy_id, $master_record_key,
   $member_count, $proposal, $field_changes, $related_counts, $related_counts_state)
ON CONFLICT (dataset_id, fingerprint) DO NOTHING
RETURNING id, org_id, dataset_id, salesforce_connection_id, object, entity_id, fingerprint, format_version, lake_snapshot, origin_run_id, writeback_policy_id, master_record_key, member_count, proposal, field_changes, related_counts, related_counts_state, status, decided_by, decided_at, decision_reason, created_at, updated_at
