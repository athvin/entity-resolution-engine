INSERT INTO public.pair_labels
  (id, org_id, rec_a_key, rec_b_key, verdict, origin, labeled_by,
   model_version, score_at_label, note)
VALUES
  ($label_id, $org_id, $rec_a_key, $rec_b_key, $verdict, $origin, $labeled_by,
   $model_version, $score_at_label, $note)
ON CONFLICT (org_id, rec_a_key, rec_b_key) DO UPDATE SET
  verdict = EXCLUDED.verdict,
  origin = EXCLUDED.origin,
  labeled_by = EXCLUDED.labeled_by,
  model_version = EXCLUDED.model_version,
  score_at_label = EXCLUDED.score_at_label,
  note = EXCLUDED.note,
  updated_at = now()
RETURNING id, org_id, rec_a_key, rec_b_key, verdict, origin, labeled_by,
          model_version, score_at_label, note, created_at, updated_at
