SELECT id, subject_type, rec_a_key, rec_b_key, entity_id, reason,
       match_probability, waterfall, status, first_seen_run_id, last_seen_run_id,
       resolved_by, resolved_at
FROM lake.main.review_queue AT (VERSION => $snapshot)
WHERE id = $review_id
