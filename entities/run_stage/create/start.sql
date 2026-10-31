INSERT INTO lake.main.run_stages
  (id, run_id, stage, seq, status, started_at, snapshot_start, try_number, task_instance_id, created_at, updated_at)
VALUES ($id, $run_id, $stage, $seq, 'running', $started_at::timestamp, $snapshot_start, $try_number,
        $task_instance_id, $started_at::timestamp, $started_at::timestamp)
