INSERT INTO lake.main.runs (run_id, tenant, mode, status, started_at, config_hash,
  model_version, tf_snapshot_id, std_version, survivorship_version, code_version,
  created_at, updated_at)
VALUES ($run_id, $tenant, $mode, 'running', $started_at::timestamp, $config_hash,
  $model_version, $tf_snapshot_id, $std_version, $survivorship_version, $code_version,
  $started_at::timestamp, $started_at::timestamp)
