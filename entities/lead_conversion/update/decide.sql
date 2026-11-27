UPDATE public.lead_conversions
SET status = $to_status, decided_by = $decided_by, decided_at = now(),
    decision_reason = $decision_reason, updated_at = now()
WHERE id = $id AND org_id = $org_id AND dataset_id = $dataset_id
  AND fingerprint = $fingerprint AND status = 'planned'
RETURNING id, org_id, dataset_id, salesforce_connection_id, entity_id, lead_external_id, lead_record_key, lead_modstamp, contact_external_id, contact_record_key, contact_modstamp, fingerprint, lake_snapshot, origin_run_id, lead_writeback_policy_id, contact_writeback_policy_id, status, decided_by, decided_at, decision_reason, created_at, updated_at
