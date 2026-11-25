UPDATE lake.main.runs SET status = 'succeeded', ended_at = $ended_at::timestamp,
    snapshot_end = $snapshot_end, updated_at = $ended_at::timestamp
WHERE id = $run_id
