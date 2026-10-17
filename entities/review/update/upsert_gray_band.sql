MERGE INTO lake.main.review_queue AS target USING (
  SELECT 'gray_band:' || rec_a_key || ':' || rec_b_key AS review_id,
         'pair' AS subject_type, rec_a_key, rec_b_key,
         CAST(NULL AS varchar) AS entity_id, 'gray_band' AS reason,
         match_probability, evidence AS waterfall, 'open' AS status,
         $run_id AS first_seen_run_id, $run_id AS last_seen_run_id,
         CAST(NULL AS varchar) AS resolved_by, CAST(NULL AS timestamp) AS resolved_at
  FROM lake.main.match_scores
  WHERE is_active AND match_probability >= $review_low AND match_probability < $auto_merge
) AS source
  ON target.rec_a_key = source.rec_a_key AND target.rec_b_key = source.rec_b_key
 AND target.reason = source.reason
WHEN MATCHED THEN UPDATE SET match_probability = source.match_probability,
    waterfall = source.waterfall, last_seen_run_id = source.last_seen_run_id
WHEN NOT MATCHED THEN INSERT
    (review_id, subject_type, rec_a_key, rec_b_key, entity_id, reason,
     match_probability, waterfall, status, first_seen_run_id, last_seen_run_id,
     resolved_by, resolved_at)
    VALUES
    (source.review_id, source.subject_type, source.rec_a_key, source.rec_b_key,
     source.entity_id, source.reason, source.match_probability, source.waterfall,
     source.status, source.first_seen_run_id, source.last_seen_run_id,
     source.resolved_by, source.resolved_at)
