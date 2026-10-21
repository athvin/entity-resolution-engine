SELECT id, org_id, rec_a_key, rec_b_key, verdict, origin, labeled_by,
       model_version, score_at_label, note, created_at, updated_at
FROM public.pair_labels
WHERE org_id = $org_id
ORDER BY rec_a_key, rec_b_key
