SELECT
  id,
  org_id,
  name,
  version,
  entity_type,
  model_json,
  model_hash,
  model_version_label,
  tf_snapshot_id,
  state,
  provenance,
  created_by,
  activated_at,
  created_at,
  updated_at
FROM public.matching_models
WHERE org_id = $org_id
ORDER BY name, version DESC
