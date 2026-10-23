SELECT id, org_id, tag_id, subject_type, subject_id, created_by, created_at, updated_at
FROM public.taggings
WHERE org_id = $org_id
  AND tag_id = $tag_id
  AND ($subject_type::varchar IS NULL OR subject_type = $subject_type::varchar)
ORDER BY subject_type, subject_id
LIMIT $limit
