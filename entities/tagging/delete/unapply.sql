DELETE FROM public.taggings
WHERE dataset_id = $dataset_id
  AND tag_id = $tag_id
  AND subject_type = $subject_type
  AND subject_id = $subject_id
