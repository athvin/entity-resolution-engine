DELETE FROM public.taggings
WHERE org_id = $org_id
  AND tag_id = $tag_id
  AND subject_type = $subject_type
  AND subject_id = $subject_id
