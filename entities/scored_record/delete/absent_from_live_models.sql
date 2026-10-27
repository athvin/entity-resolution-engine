DELETE FROM lake.main.scored_records
WHERE NOT EXISTS (
  SELECT 1 FROM __live_models m
   WHERE m.model_version = lake.main.scored_records.model_version
     AND m.tf_snapshot_id = lake.main.scored_records.tf_snapshot_id
)
