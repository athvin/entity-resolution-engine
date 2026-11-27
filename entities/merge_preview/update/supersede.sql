UPDATE public.merge_previews
SET status = 'superseded', updated_at = now()
WHERE org_id = $org_id AND dataset_id = $dataset_id
  AND salesforce_connection_id = $salesforce_connection_id AND object = $object
  AND entity_id = $entity_id AND status = 'planned' AND fingerprint <> $fingerprint
RETURNING id, org_id, dataset_id, salesforce_connection_id, object, entity_id, fingerprint, format_version, lake_snapshot, origin_run_id, writeback_policy_id, master_record_key, member_count, score, proposal, field_changes, related_counts, related_counts_state, status, decided_by, decided_at, decision_reason, created_at, updated_at
