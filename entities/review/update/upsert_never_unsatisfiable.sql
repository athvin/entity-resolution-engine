MERGE INTO lake.main.review_queue AS target USING (
  SELECT 'never_unsatisfiable:' || $rec_a_key || ':' || $rec_b_key AS review_id,
         'pair' AS subject_type, $rec_a_key AS rec_a_key, $rec_b_key AS rec_b_key,
         CAST(NULL AS varchar) AS entity_id, 'never_unsatisfiable' AS reason,
         CAST(NULL AS double) AS match_probability, CAST(NULL AS json) AS waterfall,
         'open' AS status, $run_id AS first_seen_run_id, $run_id AS last_seen_run_id,
         CAST(NULL AS varchar) AS resolved_by, CAST(NULL AS timestamp) AS resolved_at
) AS source
  ON target.rec_a_key = source.rec_a_key AND target.rec_b_key = source.rec_b_key
 AND target.reason = source.reason
WHEN MATCHED THEN UPDATE SET last_seen_run_id = source.last_seen_run_id
WHEN NOT MATCHED THEN INSERT
    (review_id, subject_type, rec_a_key, rec_b_key, entity_id, reason,
     match_probability, waterfall, status, first_seen_run_id, last_seen_run_id,
     resolved_by, resolved_at)
    VALUES
    (source.review_id, source.subject_type, source.rec_a_key, source.rec_b_key,
     source.entity_id, source.reason, source.match_probability, source.waterfall,
     source.status, source.first_seen_run_id, source.last_seen_run_id,
     source.resolved_by, source.resolved_at)
