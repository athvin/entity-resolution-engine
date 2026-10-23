UPDATE public.tags
SET active = false, updated_at = now()
WHERE org_id = $org_id AND id = $tag_id AND active
