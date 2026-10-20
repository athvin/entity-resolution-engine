INSERT INTO lake.main.runs (id, tenant, mode, status, started_at, config_hash,
  model_version, tf_snapshot_id, std_version, survivorship_version, lexicon_hash,
  code_version, rebuild_reason, created_at, updated_at)
VALUES ($run_id, $tenant, $mode, 'running', $started_at::timestamp, $config_hash,
  $model_version, $tf_snapshot_id, $std_version, $survivorship_version, $lexicon_hash,
  $code_version, $rebuild_reason, $started_at::timestamp, $started_at::timestamp)
