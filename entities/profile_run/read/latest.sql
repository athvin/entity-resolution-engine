SELECT id, subject, source_system, snapshot, tier, ingest_batch_id, corpus_digest,
       row_count, column_count, config_hash, model_version, sampled,
       sample_proportion, sample_seed, duration_ms, captured_at
FROM lake.main.profile_runs AT (VERSION => $snapshot)
WHERE subject = $subject
  AND source_system = $source_system
  AND tier = $tier
ORDER BY captured_at DESC, id DESC
LIMIT 1
