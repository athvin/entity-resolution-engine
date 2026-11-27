SELECT p.id, p.org_id, p.dataset_id, p.salesforce_connection_id, p.object, p.entity_id, p.fingerprint, p.format_version, p.lake_snapshot, p.origin_run_id, p.writeback_policy_id, p.master_record_key, p.member_count, p.proposal, p.field_changes, p.related_counts, p.related_counts_state, p.status, p.decided_by, p.decided_at, p.decision_reason, p.created_at, p.updated_at
FROM public.merge_previews p
WHERE p.org_id = $org_id AND p.dataset_id = $dataset_id AND p.status = 'approved'
  AND NOT EXISTS (SELECT 1 FROM public.merge_applies a WHERE a.merge_preview_id = p.id)
ORDER BY p.id
LIMIT $limit
