INSERT INTO public.tags (id, org_id, name, description, kind, apply_min_role, created_by)
VALUES ($tag_id, $org_id, $name, $description, $kind, $apply_min_role, $created_by)
RETURNING id, org_id, name, description, kind, apply_min_role, active, created_by, created_at, updated_at
