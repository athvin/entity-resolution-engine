UPDATE lake.main.match_scores AS target
   SET is_active = false,
       invalidated_at = stamp.invalidated_at,
       invalidated_run_id = stamp.run_id,
       updated_at = stamp.invalidated_at
  FROM __retire_stamp AS stamp
 WHERE target.is_active
   AND NOT EXISTS (
         SELECT 1 FROM __live_models AS m
          WHERE m.model_version = target.model_version
            AND m.tf_snapshot_id = target.tf_snapshot_id)
