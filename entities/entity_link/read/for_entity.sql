SELECT
  id,
  org_id,
  parent_entity_id,
  child_entity_id,
  kind,
  origin,
  note,
  created_by,
  created_at,
  updated_at
FROM public.entity_links
WHERE org_id = $org_id
  AND (parent_entity_id = $entity_id OR child_entity_id = $entity_id)
ORDER BY kind, parent_entity_id, child_entity_id
