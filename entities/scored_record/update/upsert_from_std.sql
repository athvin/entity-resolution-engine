MERGE INTO lake.main.scored_records AS target USING __scored_src AS source
  ON target.record_key = source.record_key
WHEN MATCHED THEN UPDATE SET
    content_hash = source.content_hash,
    model_version = source.model_version,
    tf_snapshot_id = source.tf_snapshot_id,
    run_id = source.run_id,
    scored_at = source.scored_at,
    updated_at = source.scored_at
WHEN NOT MATCHED THEN INSERT
    (id, record_key, content_hash, model_version, tf_snapshot_id,
     run_id, scored_at, created_at, updated_at)
    VALUES
    (sha256('scored_records:' || source.record_key),
     source.record_key, source.content_hash, source.model_version,
     source.tf_snapshot_id, source.run_id, source.scored_at,
     source.scored_at, source.scored_at)
