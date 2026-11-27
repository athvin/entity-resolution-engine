SELECT p.id, p.org_id, p.dataset_id, p.salesforce_connection_id, p.entity_id, p.lead_external_id, p.lead_record_key, p.lead_modstamp, p.contact_external_id, p.contact_record_key, p.contact_modstamp, p.fingerprint, p.lake_snapshot, p.origin_run_id, p.lead_writeback_policy_id, p.contact_writeback_policy_id, p.status, p.decided_by, p.decided_at, p.decision_reason, p.created_at, p.updated_at
FROM public.lead_conversions p
WHERE p.org_id = $org_id AND p.dataset_id = $dataset_id AND p.status = 'approved'
  AND NOT EXISTS (SELECT 1 FROM public.lead_conversion_applies a WHERE a.lead_conversion_id = p.id)
ORDER BY p.id
LIMIT $limit
