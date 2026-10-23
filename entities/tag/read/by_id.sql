SELECT id, org_id, name, description, active, created_by, created_at, updated_at
FROM public.tags
WHERE org_id = $org_id AND id = $tag_id AND active
