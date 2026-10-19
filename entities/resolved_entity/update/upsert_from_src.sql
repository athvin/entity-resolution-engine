MERGE INTO lake.main.entities AS target USING __ent_src AS source
  ON target.id = source.entity_id
WHEN MATCHED THEN UPDATE SET status = source.status,
    merged_into = source.merged_into, updated_at = $updated_at::timestamp,
    updated_run_id = $run_id
WHEN NOT MATCHED THEN INSERT
    (id, status, merged_into, created_at, updated_at, created_run_id, updated_run_id)
    VALUES (source.entity_id, source.status, source.merged_into,
            $updated_at::timestamp, $updated_at::timestamp, $run_id, $run_id)
