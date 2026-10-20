INSERT INTO public.matching_models
  (id, org_id, name, version, entity_type, model_json, model_hash,
   model_version_label, tf_snapshot_id, provenance, created_by)
VALUES
  ($model_id, $org_id, $name,
   coalesce((SELECT max(version) FROM public.matching_models
             WHERE org_id = $org_id AND name = $name), 0) + 1,
   $entity_type, $model_json, $model_hash,
   $model_version_label, $tf_snapshot_id, $provenance, $created_by)
RETURNING
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
