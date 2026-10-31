SELECT id, task_instance_id, snapshot_start, snapshot_end, counters
FROM lake.main.run_stages
WHERE run_id = $run_id AND stage = $stage AND status = 'succeeded'
  AND task_instance_id IN (SELECT unnest(string_split($task_instance_ids, chr(31))))
