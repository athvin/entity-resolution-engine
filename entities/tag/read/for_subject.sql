SELECT t.id, t.org_id, t.name, t.description, t.active, t.created_by, t.created_at, t.updated_at
FROM public.tags t
JOIN public.taggings g ON g.tag_id = t.id AND g.org_id = t.org_id
WHERE t.org_id = $org_id
  AND g.subject_type = $subject_type
  AND g.subject_id = $subject_id
  AND t.active
ORDER BY t.name
