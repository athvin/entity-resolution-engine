MERGE INTO lake.main.entity_membership AS target USING __mem_src AS source
  ON target.source_system = source.source_system
 AND target.source_record_id = source.source_record_id
WHEN MATCHED THEN UPDATE SET record_key = source.record_key,
    entity_id = source.entity_id, assigned_at = $assigned_at::timestamp, run_id = $run_id,
    updated_at = $assigned_at::timestamp
WHEN NOT MATCHED THEN INSERT
    (id, source_system, source_record_id, record_key, entity_id, assigned_at, run_id,
     created_at, updated_at)
    VALUES (sha256('entity_membership:' || source.source_system || ':' || source.source_record_id),
            source.source_system, source.source_record_id, source.record_key,
            source.entity_id, $assigned_at::timestamp, $run_id,
            $assigned_at::timestamp, $assigned_at::timestamp)
