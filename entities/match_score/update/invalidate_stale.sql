UPDATE lake.main.match_scores AS target
   SET is_active = false,
       invalidated_at = source.invalidated_at,
       invalidated_run_id = source.run_id,
       updated_at = source.invalidated_at
  FROM __match_stale AS source
 WHERE target.is_active
   AND target.model_version = source.model_version
   AND target.tf_snapshot_id = source.tf_snapshot_id
   AND (target.rec_a_key = source.record_key OR target.rec_b_key = source.record_key)
