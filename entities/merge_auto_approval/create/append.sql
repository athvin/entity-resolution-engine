INSERT INTO public.merge_auto_approvals
  (id, org_id, dataset_id, salesforce_connection_id, object, enabled, min_score, max_members, created_by)
VALUES
  ($id, $org_id, $dataset_id, $salesforce_connection_id, $object, $enabled, $min_score, $max_members, $created_by)
RETURNING id, org_id, dataset_id, salesforce_connection_id, object, enabled, min_score, max_members, created_by, created_at, updated_at
