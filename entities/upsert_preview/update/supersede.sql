UPDATE public.upsert_previews
SET status = 'superseded', updated_at = now()
WHERE org_id = $org_id AND dataset_id = $dataset_id AND entity_id = $entity_id
  AND salesforce_connection_id = $salesforce_connection_id AND object = $object
  AND status = 'planned' AND fingerprint <> $fingerprint
RETURNING id, org_id, dataset_id, salesforce_connection_id, object, entity_id, external_id_field, external_id_value, anchor_record_key, fields, fingerprint, lake_snapshot, origin_run_id, upsert_policy_id, status, decided_by, decided_at, decision_reason, created_at, updated_at
