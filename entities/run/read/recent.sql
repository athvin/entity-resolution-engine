SELECT id, mode, status, started_at, ended_at, config_hash, model_version,
       tf_snapshot_id, rebuild_reason
FROM lake.main.runs AT (VERSION => $snapshot)
ORDER BY started_at DESC
LIMIT 20
