UPDATE public.merge_previews
SET status = $to_status, decided_by = $decided_by, decided_at = now(),
    decision_reason = $decision_reason, updated_at = now()
WHERE id = $id AND org_id = $org_id AND dataset_id = $dataset_id
  AND fingerprint = $fingerprint AND status = 'planned'
RETURNING id, org_id, dataset_id, salesforce_connection_id, object, entity_id, fingerprint, format_version, lake_snapshot, origin_run_id, writeback_policy_id, master_record_key, member_count, proposal, field_changes, related_counts, related_counts_state, status, decided_by, decided_at, decision_reason, created_at, updated_at
