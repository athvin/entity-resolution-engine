MERGE INTO lake.main.metric_snapshot_sources AS target USING __metric_source_src AS source
  ON target.run_id = source.run_id AND target.source_system = source.source_system
WHEN MATCHED THEN UPDATE SET
    captured_at = source.captured_at,
    records = source.records,
    duplicate_records = source.duplicate_records,
    contributed_groups = source.contributed_groups,
    updated_at = source.captured_at
WHEN NOT MATCHED THEN INSERT
    (id, run_id, source_system, captured_at, records, duplicate_records,
     contributed_groups, created_at, updated_at)
    VALUES
    (sha256('metric_snapshot_sources:' || source.run_id || ':' || source.source_system),
     source.run_id, source.source_system, source.captured_at, source.records,
     source.duplicate_records, source.contributed_groups,
     source.captured_at, source.captured_at)
