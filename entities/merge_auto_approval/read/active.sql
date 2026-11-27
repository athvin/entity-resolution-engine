SELECT DISTINCT ON (salesforce_connection_id, object)
       id, org_id, dataset_id, salesforce_connection_id, object, enabled, min_score, max_members, created_by, created_at, updated_at
FROM public.merge_auto_approvals
WHERE org_id = $org_id AND dataset_id = $dataset_id
ORDER BY salesforce_connection_id, object, id DESC
