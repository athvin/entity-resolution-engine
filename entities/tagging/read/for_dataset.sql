SELECT id, org_id, dataset_id, tag_id, subject_type, subject_id, created_by, created_at, updated_at
FROM public.taggings
WHERE dataset_id = $dataset_id
  AND subject_type = $subject_type
ORDER BY subject_id
LIMIT $limit
