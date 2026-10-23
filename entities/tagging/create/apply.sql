INSERT INTO public.taggings (id, org_id, tag_id, subject_type, subject_id, created_by)
VALUES ($tagging_id, $org_id, $tag_id, $subject_type, $subject_id, $created_by)
ON CONFLICT (org_id, tag_id, subject_type, subject_id) DO NOTHING
