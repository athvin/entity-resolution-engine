MERGE INTO lake.main.match_scores AS target USING __match_src AS source
  ON target.rec_a_key = source.rec_a_key AND target.rec_b_key = source.rec_b_key
 AND target.model_version = source.model_version
 AND target.tf_snapshot_id = source.tf_snapshot_id
WHEN MATCHED THEN UPDATE SET
    match_probability = source.match_probability,
    rec_a_content_hash = source.rec_a_content_hash,
    rec_b_content_hash = source.rec_b_content_hash,
    evidence = source.evidence,
    is_active = true,
    invalidated_at = NULL,
    invalidated_run_id = NULL,
    run_id = source.run_id,
    scored_at = source.scored_at
WHEN NOT MATCHED THEN INSERT
    (rec_a_key, rec_b_key, match_probability, model_version, tf_snapshot_id,
     rec_a_content_hash, rec_b_content_hash, evidence, is_active,
     invalidated_at, invalidated_run_id, run_id, scored_at)
    VALUES
    (source.rec_a_key, source.rec_b_key, source.match_probability, source.model_version,
     source.tf_snapshot_id, source.rec_a_content_hash, source.rec_b_content_hash,
     source.evidence, true, NULL, NULL, source.run_id, source.scored_at)
