INSERT INTO public.tags (id, org_id, name, description, created_by)
VALUES ($tag_id, $org_id, $name, $description, $created_by)
RETURNING id, org_id, name, description, active, created_by, created_at, updated_at
