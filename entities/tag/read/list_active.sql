SELECT id, org_id, name, description, kind, apply_min_role, active, created_by, created_at, updated_at
FROM public.tags
WHERE org_id = $org_id AND active
ORDER BY name
