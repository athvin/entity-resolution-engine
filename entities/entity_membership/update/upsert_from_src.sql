MERGE INTO lake.main.entity_membership AS target USING __mem_src AS source
  ON target.source_system = source.source_system
 AND target.source_record_id = source.source_record_id
WHEN MATCHED THEN UPDATE SET record_key = source.record_key,
    entity_id = source.entity_id, assigned_at = $assigned_at::timestamp, run_id = $run_id
WHEN NOT MATCHED THEN INSERT
    (source_system, source_record_id, record_key, entity_id, assigned_at, run_id)
    VALUES (source.source_system, source.source_record_id, source.record_key,
            source.entity_id, $assigned_at::timestamp, $run_id)
