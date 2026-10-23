SELECT run_id, source_system, captured_at, records, duplicate_records,
       contributed_groups
FROM lake.main.metric_snapshot_sources AT (VERSION => $snapshot)
WHERE run_id = $run_id
ORDER BY duplicate_records DESC, source_system
