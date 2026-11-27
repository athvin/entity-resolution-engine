SELECT id, org_id, dataset_id, salesforce_connection_id, entity_id, lead_external_id, lead_record_key, lead_modstamp, contact_external_id, contact_record_key, contact_modstamp, fingerprint, lake_snapshot, origin_run_id, lead_writeback_policy_id, contact_writeback_policy_id, status, decided_by, decided_at, decision_reason, created_at, updated_at
FROM public.lead_conversions
WHERE org_id = $org_id AND dataset_id = $dataset_id
  AND ($status = '' OR status = $status)
  AND id > $after
ORDER BY id
LIMIT $limit
