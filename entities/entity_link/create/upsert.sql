INSERT INTO public.entity_links
  (id, org_id, dataset_id, parent_entity_id, child_entity_id, kind, origin, note, created_by)
VALUES
  ($link_id, $org_id, $dataset_id, $parent_entity_id, $child_entity_id, $kind, $origin,
   $note, $created_by)
ON CONFLICT (org_id, parent_entity_id, child_entity_id, kind) DO UPDATE SET
  dataset_id = EXCLUDED.dataset_id,
  origin = EXCLUDED.origin,
  note = EXCLUDED.note,
  created_by = EXCLUDED.created_by,
  updated_at = now()
RETURNING id, org_id, dataset_id, parent_entity_id, child_entity_id, kind, origin, note,
          created_by, created_at, updated_at
