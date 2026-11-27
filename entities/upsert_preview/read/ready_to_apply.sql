SELECT p.id, p.org_id, p.dataset_id, p.salesforce_connection_id, p.object, p.entity_id, p.external_id_field, p.external_id_value, p.anchor_record_key, p.fields, p.fingerprint, p.lake_snapshot, p.origin_run_id, p.upsert_policy_id, p.status, p.decided_by, p.decided_at, p.decision_reason, p.created_at, p.updated_at
FROM public.upsert_previews p
WHERE p.org_id = $org_id AND p.dataset_id = $dataset_id AND p.status = 'approved'
  AND NOT EXISTS (SELECT 1 FROM public.upsert_applies a WHERE a.upsert_preview_id = p.id)
ORDER BY p.id
LIMIT $limit
